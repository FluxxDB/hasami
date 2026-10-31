//! Export job: render the Quick Trim editor's edit and save it beside the source as
//! `[CLIPPED] <name>`.
//!
//! - **Quick export** and **Export + save raw** re-encode the edit with the source's own video
//!   codec at about its bitrate, in its own container. The original then goes to the Recycle
//!   Bin, or into a `Raw` folder next to it.
//! - With **Fit to 20 MB** they instead render a near-lossless H.264 intermediate and run the
//!   Compress pipeline on it, giving an `.mp4` of at most [`SIZE_LIMIT_BYTES`].
//! - **Export audio** mixes the kept audio into one `.mp3`; the original is untouched.
//!
//! As with every job, the result is written to a hidden temp file, verified, and only then given
//! its name; the original is touched last. On any error or cancel the temp file is deleted.

use std::ffi::OsString;
use std::path::Path;

use super::compress::{create_work_dir, encode_to_limit, RemoveOnDrop};
use super::{JobError, JobOutcome, JobProgress, UNREADABLE};
use crate::compress::{plan_compress, CompressDecision, SIZE_LIMIT_BYTES};
use crate::editor::request::{
    audio_codec_args, build_graph, encoder_available, same_codec_encoder, source_video_bitrate,
    ExportMode, ExportRequest, Graph, VideoEncoder,
};
use crate::fsops::{clipped_path, commit_new, commit_replace, move_to_raw, temp_path_for};
use crate::media::run::{run_ffmpeg, CancelToken};
use crate::media::{probe, FfmpegPaths, MediaInfo};
use crate::verify::{verify_output, VerifyError};

/// Shown for an audio export when every audio track is muted or the source has none.
const NO_AUDIO: &str = "This clip has no audio to export.";
/// Shown for a video export whose timeline has no video left in it.
const NO_VIDEO: &str = "Couldn't export this clip: there's no video left in it.";
/// Shown when the rendered result fails verification for a reason other than its size.
const DIDNT_CHECK_OUT: &str = "Couldn't export this clip: the result didn't check out.";
/// Shown when a Fit to 20 MB result is still over the limit at the final check.
const OVER_LIMIT: &str = "Couldn't get this clip under 20 MB.";
/// A Fit to 20 MB export: the intermediate, then the two Compress passes.
const FIT_PASSES: u8 = 3;

/// Render `req` and save it as `[CLIPPED] <name>` per its mode (see the module docs).
pub fn export(
    paths: &FfmpegPaths,
    req: &ExportRequest,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
) -> Result<JobOutcome, JobError> {
    export_in(paths, req, cancel, on_progress, &std::env::temp_dir())
}

/// [`export`] with the parent of the Fit to 20 MB work folder given, so tests can check that it
/// is cleaned up.
fn export_in(
    paths: &FfmpegPaths,
    req: &ExportRequest,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
    work_root: &Path,
) -> Result<JobOutcome, JobError> {
    let info = probe(paths, &req.input).map_err(|_| JobError::Message(UNREADABLE.to_owned()))?;

    let (render, graph) = if req.mode == ExportMode::Audio {
        // Video pieces would leave `[vout]` unconnected; only the audio timeline is wanted.
        let graph = build_graph(&ExportRequest { video: Vec::new(), ..req.clone() });
        if graph.audio_out.is_none() {
            return Err(JobError::Message(NO_AUDIO.to_owned()));
        }
        (Render::Mp3, graph)
    } else if req.video.is_empty() {
        return Err(JobError::Message(NO_VIDEO.to_owned()));
    } else if req.fit_20mb {
        (Render::Fit, build_graph(req))
    } else {
        (Render::SameCodec(Container::of(&info)), build_graph(req))
    };

    let final_path = clipped_path(&req.input, render.extension());
    // Deleted however this ends (a committed temp is already gone), unless a failed commit
    // left the only copy of the result there.
    let mut temp = RemoveOnDrop::file(temp_path_for(&final_path));
    let job = Job { paths, req, info: &info, graph: &graph, render, temp: temp.path(), work_root };
    let result = job.run(&final_path, cancel, on_progress);
    if result.as_ref().is_err_and(JobError::keeps_new_file) {
        temp.keep();
    }
    result
}

/// How the edit is turned into a file.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Render {
    /// The source's video codec, in the source's container.
    SameCodec(Container),
    /// Intermediate, then the Compress pipeline: H.264/AAC `.mp4` within the size limit.
    Fit,
    /// The audio mix alone, as MP3.
    Mp3,
}

impl Render {
    fn extension(self) -> &'static str {
        match self {
            Render::SameCodec(c) => c.ext,
            Render::Fit => "mp4",
            Render::Mp3 => "mp3",
        }
    }
}

/// An output container: the file extension and ffmpeg's muxer name for it. The muxer is always
/// given with `-f`, since the temp file's `.hasami-tmp` extension says nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Container {
    ext: &'static str,
    muxer: &'static str,
}

impl Container {
    /// The source's own container, from its extension (which is what `MediaInfo::container`
    /// holds). Anything Hasami doesn't register for is written as `.mp4`.
    fn of(info: &MediaInfo) -> Self {
        let (ext, muxer) = match info.container.as_str() {
            "mov" => ("mov", "mov"),
            "mkv" => ("mkv", "matroska"),
            "webm" => ("webm", "webm"),
            _ => ("mp4", "mp4"),
        };
        Container { ext, muxer }
    }

    /// MP4 and MOV get their index up front, so the clip starts playing before it fully loads.
    fn faststart(self) -> bool {
        matches!(self.ext, "mp4" | "mov")
    }
}

/// Everything one export needs once the request has been read.
struct Job<'a> {
    paths: &'a FfmpegPaths,
    req: &'a ExportRequest,
    info: &'a MediaInfo,
    graph: &'a Graph,
    render: Render,
    temp: &'a Path,
    work_root: &'a Path,
}

impl Job<'_> {
    /// Render into the temp file, verify it, then commit it as `final_path` per the mode.
    fn run(
        &self,
        final_path: &Path,
        cancel: &CancelToken,
        on_progress: &mut dyn FnMut(JobProgress),
    ) -> Result<JobOutcome, JobError> {
        let input = self.req.input.as_path();
        match self.render {
            Render::SameCodec(container) => {
                let args = same_codec_args(input, self.graph, &self.encoder(container), container, self.temp);
                self.one_pass(&args, cancel, on_progress)?;
            }
            Render::Mp3 => self.one_pass(&mp3_args(input, self.graph, self.temp), cancel, on_progress)?,
            Render::Fit => self.fit(cancel, on_progress)?,
        }

        let max_bytes = (self.render == Render::Fit).then_some(SIZE_LIMIT_BYTES);
        let verified =
            verify_output(self.paths, self.temp, self.req.duration_s, max_bytes).map_err(plain_verify_error)?;

        // Last chance to back out; once the commit starts it runs to completion or rolls back.
        if cancel.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        match self.req.mode {
            ExportMode::Quick => commit_replace(self.temp, input, final_path)?,
            ExportMode::SaveRaw => {
                commit_new(self.temp, final_path)?;
                // The new file is saved either way; only the original's move can still fail.
                move_to_raw(input).map_err(|_| JobError::Message(raw_move_failed(final_path)))?;
            }
            ExportMode::Audio => commit_new(self.temp, final_path)?,
        }
        Ok(JobOutcome::Done { output: final_path.to_path_buf(), bytes: verified.size_bytes })
    }

    /// The source's codec at about its video bitrate, on AMD hardware when it works here.
    fn encoder(&self, container: Container) -> VideoEncoder {
        let codec = self.info.video.as_ref().map_or("h264", |v| v.codec.as_str());
        let bitrate = source_video_bitrate(self.info);
        same_codec_encoder(codec, container.ext, bitrate, &|name| encoder_available(self.paths, name))
    }

    /// Run a single ffmpeg pass over the edit, reported as pass 1 of 1.
    fn one_pass(
        &self,
        args: &[OsString],
        cancel: &CancelToken,
        on_progress: &mut dyn FnMut(JobProgress),
    ) -> Result<(), JobError> {
        run_ffmpeg(self.paths, args, self.req.duration_s, cancel, &mut |fraction| {
            on_progress(JobProgress { pass: 1, passes: 1, fraction })
        })?;
        Ok(())
    }

    /// Fit to 20 MB: render the edit to an intermediate in a fresh work folder (pass 1), then
    /// run the two Compress passes on it (passes 2–3), or, if it already fits, just remux it
    /// (reported as pass 3, so the bar still ends full).
    /// The work folder is removed however this ends.
    fn fit(&self, cancel: &CancelToken, on_progress: &mut dyn FnMut(JobProgress)) -> Result<(), JobError> {
        let work = RemoveOnDrop::dir(create_work_dir(self.work_root, "hasami-export")?);
        let intermediate = work.path().join("edit.mp4");
        let at_pass = |pass: u8, fraction: f64| JobProgress { pass, passes: FIT_PASSES, fraction };

        let args = intermediate_args(&self.req.input, self.graph, &intermediate);
        run_ffmpeg(self.paths, &args, self.req.duration_s, cancel, &mut |f| on_progress(at_pass(1, f)))?;
        let info = probe(self.paths, &intermediate).map_err(|_| JobError::Message(DIDNT_CHECK_OUT.to_owned()))?;

        match plan_compress(&info)? {
            CompressDecision::AlreadySmall => {
                let args = remux_args(&intermediate, self.temp);
                run_ffmpeg(self.paths, &args, info.duration_s, cancel, &mut |f| on_progress(at_pass(3, f)))?;
            }
            CompressDecision::Encode(plan) => {
                // Its passes 1–2 are this export's passes 2–3.
                encode_to_limit(
                    self.paths,
                    &intermediate,
                    &info,
                    &plan,
                    self.temp,
                    work.path(),
                    cancel,
                    &mut |p| on_progress(at_pass(p.pass + 1, p.fraction)),
                )?;
            }
        }
        Ok(())
    }
}

/// `Saved "<name>", but couldn't move the original into the Raw folder.`
fn raw_move_failed(final_path: &Path) -> String {
    let name = final_path.file_name().unwrap_or_default().to_string_lossy();
    format!("Saved \"{name}\", but couldn't move the original into the Raw folder.")
}

/// Replace a verification failure's technical text with plain language for the user.
fn plain_verify_error(e: VerifyError) -> JobError {
    let message = match e {
        VerifyError::TooLarge { .. } => OVER_LIMIT,
        VerifyError::Probe(_) | VerifyError::WrongDuration { .. } => DIDNT_CHECK_OUT,
    };
    JobError::Message(message.to_owned())
}

fn push(args: &mut Vec<OsString>, items: &[&str]) {
    args.extend(items.iter().map(OsString::from));
}

/// `-i <input> -filter_complex <graph>`, a `-map` for each of the graph's outputs, and `-an`
/// when it has no audio.
fn graph_args(input: &Path, graph: &Graph) -> Vec<OsString> {
    let mut a = vec![OsString::from("-i"), input.as_os_str().to_owned()];
    push(&mut a, &["-filter_complex", &graph.filter]);
    if let Some(v) = &graph.video_out {
        push(&mut a, &["-map", v]);
    }
    match &graph.audio_out {
        Some(aout) => push(&mut a, &["-map", aout]),
        None => push(&mut a, &["-an"]),
    }
    a
}

/// Quick / Save raw without fit: the source's codec and container.
fn same_codec_args(
    input: &Path,
    graph: &Graph,
    encoder: &VideoEncoder,
    container: Container,
    out: &Path,
) -> Vec<OsString> {
    let mut a = graph_args(input, graph);
    push(&mut a, &["-c:v", encoder.codec]);
    a.extend(encoder.args.iter().map(OsString::from));
    if graph.audio_out.is_some() {
        a.extend(audio_codec_args(container.ext).into_iter().map(OsString::from));
    }
    if container.faststart() {
        push(&mut a, &["-movflags", "+faststart"]);
    }
    push(&mut a, &["-f", container.muxer]);
    a.push(out.as_os_str().to_owned());
    a
}

/// Export audio: the mix as VBR MP3.
fn mp3_args(input: &Path, graph: &Graph, out: &Path) -> Vec<OsString> {
    let mut a = graph_args(input, graph);
    push(&mut a, &["-c:a", "libmp3lame", "-q:a", "2", "-f", "mp3"]);
    a.push(out.as_os_str().to_owned());
    a
}

/// Fit pass 1: the edit at near-source quality, for the Compress pipeline to work from.
/// 8-bit 4:2:0 so that a remux of it (when it already fits) plays everywhere.
fn intermediate_args(input: &Path, graph: &Graph, out: &Path) -> Vec<OsString> {
    let mut a = graph_args(input, graph);
    push(&mut a, &["-c:v", "libx264", "-preset", "veryfast", "-crf", "14", "-pix_fmt", "yuv420p"]);
    if graph.audio_out.is_some() {
        push(&mut a, &["-c:a", "aac", "-b:a", "320k"]);
    }
    push(&mut a, &["-f", "mp4"]);
    a.push(out.as_os_str().to_owned());
    a
}

/// Fit when the intermediate is already small enough: copy it into the temp file.
fn remux_args(input: &Path, out: &Path) -> Vec<OsString> {
    let mut a = vec![OsString::from("-i"), input.as_os_str().to_owned()];
    push(&mut a, &["-map", "0", "-c", "copy", "-movflags", "+faststart", "-f", "mp4"]);
    a.push(out.as_os_str().to_owned());
    a
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::editor::request::{AudioPiece, AudioTrackRequest, VideoPiece};
    use crate::media::hidden_command;

    fn text(args: &[OsString]) -> String {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
    }

    fn graph(audio: bool) -> Graph {
        Graph {
            filter: "G".to_owned(),
            video_out: Some("[vout]".to_owned()),
            audio_out: audio.then(|| "[aout]".to_owned()),
        }
    }

    fn container(ext: &str) -> Container {
        Container::of(&MediaInfo {
            path: PathBuf::from(format!("in.{ext}")),
            container: ext.to_owned(),
            duration_s: 10.0,
            size_bytes: 1,
            bit_rate: None,
            video: None,
            audio: Vec::new(),
        })
    }

    fn encoder() -> VideoEncoder {
        VideoEncoder { codec: "hevc_amf", args: vec!["-b:v".to_owned(), "9000000".to_owned()] }
    }

    fn same_codec(ext: &str, audio: bool) -> String {
        let input = PathBuf::from(format!("in.{ext}"));
        text(&same_codec_args(&input, &graph(audio), &encoder(), container(ext), Path::new("tmp")))
    }

    #[test]
    fn containers_map_to_their_muxers() {
        assert_eq!(container("mp4"), Container { ext: "mp4", muxer: "mp4" });
        assert_eq!(container("mov"), Container { ext: "mov", muxer: "mov" });
        assert_eq!(container("mkv"), Container { ext: "mkv", muxer: "matroska" });
        assert_eq!(container("webm"), Container { ext: "webm", muxer: "webm" });
        assert_eq!(container("avi"), Container { ext: "mp4", muxer: "mp4" });
    }

    #[test]
    fn same_codec_maps_the_graph_and_names_the_muxer() {
        assert_eq!(
            same_codec("mkv", true),
            "-i in.mkv -filter_complex G -map [vout] -map [aout] -c:v hevc_amf -b:v 9000000 \
             -c:a aac -b:a 192k -f matroska tmp"
        );
    }

    #[test]
    fn same_codec_without_audio_drops_it() {
        let a = same_codec("mp4", false);
        assert!(a.contains("-map [vout] -an "), "{a}");
        assert!(!a.contains("-c:a"), "{a}");
        assert!(a.ends_with("-movflags +faststart -f mp4 tmp"), "{a}");
    }

    #[test]
    fn webm_gets_opus() {
        let a = same_codec("webm", true);
        assert!(a.ends_with("-c:a libopus -b:a 160k -f webm tmp"), "{a}");
    }

    #[test]
    fn mp3_and_intermediate_args() {
        let audio_only = Graph { video_out: None, ..graph(true) };
        assert_eq!(
            text(&mp3_args(Path::new("in.mp4"), &audio_only, Path::new("tmp"))),
            "-i in.mp4 -filter_complex G -map [aout] -c:a libmp3lame -q:a 2 -f mp3 tmp"
        );
        assert_eq!(
            text(&intermediate_args(Path::new("in.mp4"), &graph(true), Path::new("edit.mp4"))),
            "-i in.mp4 -filter_complex G -map [vout] -map [aout] -c:v libx264 -preset veryfast -crf 14 \
             -pix_fmt yuv420p -c:a aac -b:a 320k -f mp4 edit.mp4"
        );
    }

    #[test]
    fn errors_read_as_plain_language() {
        let wrong = VerifyError::WrongDuration { expected_s: 3.0, actual_s: 2.0 };
        assert_eq!(plain_verify_error(wrong).to_string(), DIDNT_CHECK_OUT);
        let big = VerifyError::TooLarge { size_bytes: 21_000_000, max_bytes: SIZE_LIMIT_BYTES };
        assert_eq!(plain_verify_error(big).to_string(), OVER_LIMIT);
        assert_eq!(
            raw_move_failed(Path::new(r"C:\d\[CLIPPED] clip.mp4")),
            "Saved \"[CLIPPED] clip.mp4\", but couldn't move the original into the Raw folder."
        );
    }

    /// 2 s of small H.264 + AAC in an `.mkv` inside `dir`.
    fn small_clip(paths: &FfmpegPaths, dir: &Path) -> PathBuf {
        let out = dir.join("clip.mkv");
        let status = hidden_command(&paths.ffmpeg)
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=2"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"])
            .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac"])
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success());
        out
    }

    fn fit_request(input: &Path) -> ExportRequest {
        ExportRequest {
            session: 1,
            input: input.to_path_buf(),
            mode: ExportMode::SaveRaw,
            fit_20mb: true,
            duration_s: 1.5,
            video: vec![VideoPiece { src_start: 0.5, src_end: 2.0 }],
            audio: vec![AudioTrackRequest {
                index: 0,
                gain_db: 0.0,
                pieces: vec![AudioPiece { at: 0.0, src_start: 0.5, src_end: 2.0 }],
            }],
        }
    }

    #[test]
    fn fit_work_folder_is_removed_after_cancel_and_success() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let empty = |root: &Path| fs::read_dir(root).unwrap().next().is_none();
        let input = small_clip(&paths, dir.path());

        let cancel = CancelToken::new();
        let outcome = export_in(&paths, &fit_request(&input), &cancel, &mut |_| cancel.cancel(), root.path());
        assert!(matches!(outcome, Err(JobError::Cancelled)), "{outcome:?}");
        assert!(empty(root.path()), "work folder left behind after cancel");
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["clip.mkv"], "only the untouched input may remain");

        let mut passes = Vec::new();
        let mut record = |p: JobProgress| passes.push((p.pass, p.passes));
        let outcome = export_in(&paths, &fit_request(&input), &CancelToken::new(), &mut record, root.path());
        let Ok(JobOutcome::Done { output, .. }) = outcome else { panic!("{outcome:?}") };
        assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
        assert!(empty(root.path()), "work folder left behind after success");
        // A small edit already fits, so the intermediate is just remuxed, as the last pass.
        assert!(passes.contains(&(1, 3)) && passes.contains(&(3, 3)), "{passes:?}");
        assert!(passes.iter().all(|&(pass, of)| of == 3 && pass != 2), "{passes:?}");
    }

    #[test]
    fn video_export_without_video_is_refused() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = small_clip(&paths, dir.path());
        for (mode, fit) in [(ExportMode::Quick, false), (ExportMode::SaveRaw, true)] {
            let req = ExportRequest { mode, fit_20mb: fit, video: Vec::new(), ..fit_request(&input) };
            let mut ran = false;
            let err = export_in(&paths, &req, &CancelToken::new(), &mut |_| ran = true, dir.path()).unwrap_err();
            assert_eq!(err.to_string(), NO_VIDEO);
            assert!(!ran, "ffmpeg must not run");
        }
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["clip.mkv"]);
    }
}
