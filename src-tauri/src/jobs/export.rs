//! Export job: render the Quick Trim editor's edit and save it beside the source as
//! `[CLIPPED] <name>`.
//!
//! - **Quick export** and **Export + save raw** re-encode the edit with the source's own video
//!   codec at about its bitrate, in its own container. The original then goes to the Recycle
//!   Bin, or into a `Raw` folder next to it.
//! - With the **20 MB copy** toggle on, they then also save `[20MB] <name>.mp4` (at most
//!   [`SIZE_LIMIT_BYTES`]) in the Windows Videos folder, made from the saved `[CLIPPED]` file by
//!   the Compress pipeline, or copied as is when that file is already small and Discord-ready.
//!   A failed copy never fails the export: the outcome is still `Done`, with a note saying why.
//! - **Export audio** mixes the kept audio into one `.mp3`; the original is untouched.
//!
//! As with every job, each result is written to a hidden temp file, verified, and only then given
//! its name; the original is touched last. On any error or cancel the temp file is deleted.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use super::compress::{encode_to_limit, RemoveOnDrop};
use super::{JobError, JobOutcome, JobProgress, UNREADABLE};
use crate::compress::{plan_compress, CompressDecision, SIZE_LIMIT_BYTES};
use crate::editor::request::{
    audio_codec_args, build_graph, encoder_available, same_codec_encoder, source_video_bitrate,
    ExportMode, ExportRequest, Graph, VideoEncoder,
};
use crate::fsops::{
    clipped_path, commit_new, commit_replace, compressed_path, move_to_raw, temp_path_for, videos_dir,
    FsError, NO_VIDEOS_FOLDER,
};
use crate::media::run::{run_ffmpeg, CancelToken};
use crate::media::{probe, FfmpegPaths, MediaInfo};
use crate::verify::{verify_output, VerifyError};

/// Shown for an audio export when every audio track is muted or the source has none.
const NO_AUDIO: &str = "This clip has no audio to export.";
/// Shown for a video export whose timeline has no video left in it.
const NO_VIDEO: &str = "Couldn't export this clip: there's no video left in it.";
/// Shown when the rendered result fails verification for a reason other than its size.
const DIDNT_CHECK_OUT: &str = "Couldn't export this clip: the result didn't check out.";
/// Shown when the 20 MB copy is still over the limit at the final check.
const OVER_LIMIT: &str = "Couldn't get this clip under 20 MB.";
/// Shown when the already-small `[CLIPPED]` file couldn't be copied into the Videos folder.
const COPY_FAILED: &str = "Couldn't copy the file into your Videos folder.";
/// The reason in the note when the 20 MB copy was cancelled.
const COPY_CANCELLED: &str = "it was cancelled.";
/// An export with the 20 MB copy: the `[CLIPPED]` render, then the copy's two Compress passes.
const COPY_PASSES: u8 = 3;

/// Finds the folder the 20 MB copy goes into.
type VideosDirFn<'a> = &'a dyn Fn() -> Result<PathBuf, FsError>;

/// Render `req` and save it as `[CLIPPED] <name>` per its mode, plus the 20 MB copy in the
/// Windows Videos folder when asked (see the module docs).
pub fn export(
    paths: &FfmpegPaths,
    req: &ExportRequest,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
) -> Result<JobOutcome, JobError> {
    export_work(paths, req, &videos_dir, cancel, on_progress, &std::env::temp_dir())
}

/// [`export`] with the 20 MB copy going into `videos_dir` instead of the Windows Videos folder.
pub fn export_in(
    paths: &FfmpegPaths,
    req: &ExportRequest,
    videos_dir: &Path,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
) -> Result<JobOutcome, JobError> {
    let videos = || Ok(videos_dir.to_path_buf());
    export_work(paths, req, &videos, cancel, on_progress, &std::env::temp_dir())
}

/// [`export`] with the Videos folder lookup and the parent of the copy's pass-log folder given,
/// so tests can check that it is cleaned up.
fn export_work(
    paths: &FfmpegPaths,
    req: &ExportRequest,
    videos_dir: VideosDirFn,
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
    } else {
        (Render::SameCodec(Container::of(&info)), build_graph(req))
    };
    // The toggle belongs to the two video exports.
    let make_copy = req.fit_20mb && render != Render::Mp3;
    let passes = if make_copy { COPY_PASSES } else { 1 };

    let final_path = clipped_path(&req.input, render.extension());
    // Deleted however this ends (a committed temp is already gone), unless a failed commit
    // left the only copy of the result there.
    let mut temp = RemoveOnDrop::file(temp_path_for(&final_path));
    let job = Job { paths, req, info: &info, graph: &graph, render, temp: temp.path(), passes };
    let result = job.run(&final_path, cancel, on_progress);
    if result.as_ref().is_err_and(JobError::keeps_new_file) {
        temp.keep();
    }
    let bytes = result?;

    // The `[CLIPPED]` file is saved and the original dealt with: nothing the copy does can put
    // either at risk, so its failure is only a note. Its passes 1–2 are this export's 2–3.
    let note = if make_copy {
        let mut copy_progress =
            |p: JobProgress| on_progress(JobProgress { pass: p.pass + 1, passes, fraction: p.fraction });
        save_20mb_copy(paths, &final_path, videos_dir, work_root, cancel, &mut copy_progress)
            .err()
            .map(|e| copy_failed(&final_path, &e))
    } else {
        None
    };
    Ok(JobOutcome::Done { output: final_path, bytes, note })
}

/// Save `clipped` (the verified, committed export) as `[20MB] <name>.mp4` in the Videos folder:
/// copied as is when it's already small and Discord-ready, otherwise through the Compress
/// pipeline (reported as passes 1–2 of 2; a plain copy reports pass 2 done). Never touches
/// `clipped`. Returns the copy's path.
fn save_20mb_copy(
    paths: &FfmpegPaths,
    clipped: &Path,
    videos_dir: VideosDirFn,
    work_root: &Path,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
) -> Result<PathBuf, JobError> {
    let videos = videos_dir()?;
    if !videos.is_dir() {
        return Err(JobError::Message(NO_VIDEOS_FOLDER.to_owned()));
    }
    let info = probe(paths, clipped).map_err(|_| JobError::Message(UNREADABLE.to_owned()))?;
    let final_path = compressed_path(clipped, &videos);
    let mut temp = RemoveOnDrop::file(temp_path_for(&final_path));

    match plan_compress(&info)? {
        CompressDecision::AlreadySmall => {
            fs::copy(clipped, temp.path()).map_err(|_| JobError::Message(COPY_FAILED.to_owned()))?;
            verify_output(paths, temp.path(), info.duration_s, Some(SIZE_LIMIT_BYTES))
                .map_err(plain_verify_error)?;
            on_progress(JobProgress { pass: 2, passes: 2, fraction: 1.0 });
        }
        CompressDecision::Encode(plan) => {
            encode_to_limit(paths, clipped, &info, &plan, temp.path(), work_root, cancel, on_progress)?;
        }
    }

    if cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    if let Err(e) = commit_new(temp.path(), &final_path) {
        if e.kept_new_file.is_some() {
            // The rename may have happened after all; keep what may be the only copy.
            temp.keep();
        }
        return Err(e.into());
    }
    Ok(final_path)
}

/// `Saved "<name>", but couldn't make the 20 MB copy: <reason>`
fn copy_failed(saved: &Path, e: &JobError) -> String {
    let name = saved.file_name().unwrap_or_default().to_string_lossy();
    let reason = match e {
        JobError::Cancelled => COPY_CANCELLED.to_owned(),
        other => other.to_string(),
    };
    format!("Saved \"{name}\", but couldn't make the 20 MB copy: {reason}")
}

/// How the edit is turned into a file.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Render {
    /// The source's video codec, in the source's container.
    SameCodec(Container),
    /// The audio mix alone, as MP3.
    Mp3,
}

impl Render {
    fn extension(self) -> &'static str {
        match self {
            Render::SameCodec(c) => c.ext,
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
    /// Passes in the whole job; the render is always pass 1.
    passes: u8,
}

impl Job<'_> {
    /// Render into the temp file, verify it, then commit it as `final_path` per the mode.
    /// Returns the saved file's size.
    fn run(
        &self,
        final_path: &Path,
        cancel: &CancelToken,
        on_progress: &mut dyn FnMut(JobProgress),
    ) -> Result<u64, JobError> {
        let input = self.req.input.as_path();
        let args = match self.render {
            Render::SameCodec(container) => {
                same_codec_args(input, self.graph, &self.encoder(container), container, self.temp)
            }
            Render::Mp3 => mp3_args(input, self.graph, self.temp),
        };
        run_ffmpeg(self.paths, &args, self.req.duration_s, cancel, &mut |fraction| {
            on_progress(JobProgress { pass: 1, passes: self.passes, fraction })
        })?;

        let verified =
            verify_output(self.paths, self.temp, self.req.duration_s, None).map_err(plain_verify_error)?;

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
        Ok(verified.size_bytes)
    }

    /// The source's codec at about its video bitrate, on AMD hardware when it works here.
    fn encoder(&self, container: Container) -> VideoEncoder {
        let codec = self.info.video.as_ref().map_or("h264", |v| v.codec.as_str());
        let bitrate = source_video_bitrate(self.info);
        same_codec_encoder(codec, container.ext, bitrate, &|name| encoder_available(self.paths, name))
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

#[cfg(test)]
mod tests {
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
    fn mp3_args_are_vbr_mp3() {
        let audio_only = Graph { video_out: None, ..graph(true) };
        assert_eq!(
            text(&mp3_args(Path::new("in.mp4"), &audio_only, Path::new("tmp"))),
            "-i in.mp4 -filter_complex G -map [aout] -c:a libmp3lame -q:a 2 -f mp3 tmp"
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
        let saved = Path::new(r"C:\d\[CLIPPED] clip.mp4");
        assert_eq!(
            copy_failed(saved, &JobError::Message("Couldn't get this video under 20 MB.".into())),
            "Saved \"[CLIPPED] clip.mp4\", but couldn't make the 20 MB copy: Couldn't get this video under 20 MB."
        );
        assert_eq!(
            copy_failed(saved, &JobError::Cancelled),
            "Saved \"[CLIPPED] clip.mp4\", but couldn't make the 20 MB copy: it was cancelled."
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
            master_db: 0.0,
        }
    }

    /// The names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_cancelled_render_saves_nothing() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let videos = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let input = small_clip(&paths, dir.path());
        let lookup = || Ok(videos.path().to_path_buf());

        let cancel = CancelToken::new();
        let outcome =
            export_work(&paths, &fit_request(&input), &lookup, &cancel, &mut |_| cancel.cancel(), root.path());
        assert!(matches!(outcome, Err(JobError::Cancelled)), "{outcome:?}");
        assert_eq!(names(dir.path()), ["clip.mkv"], "only the untouched input may remain");
        assert!(names(videos.path()).is_empty());
        assert!(names(root.path()).is_empty());
    }

    #[test]
    fn copy_pass_log_is_removed_after_cancel_and_success() {
        let paths = FfmpegPaths::resolve().unwrap();
        let videos = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let lookup = || Ok(videos.path().to_path_buf());

        // Cancelled during the copy: the [CLIPPED] file is saved and the original moved already,
        // so the export is still done, with a note.
        let dir = tempfile::tempdir().unwrap();
        let input = small_clip(&paths, dir.path());
        let cancel = CancelToken::new();
        let mut cancel_in_copy = |p: JobProgress| {
            if p.pass >= 2 {
                cancel.cancel();
            }
        };
        let outcome = export_work(&paths, &fit_request(&input), &lookup, &cancel, &mut cancel_in_copy, root.path());
        let Ok(JobOutcome::Done { output, note, .. }) = outcome else { panic!("{outcome:?}") };
        assert_eq!(output, dir.path().join("[CLIPPED] clip.mkv"));
        assert_eq!(
            note.as_deref(),
            Some("Saved \"[CLIPPED] clip.mkv\", but couldn't make the 20 MB copy: it was cancelled.")
        );
        assert_eq!(names(dir.path()), ["Raw", "[CLIPPED] clip.mkv"]);
        assert!(names(videos.path()).is_empty(), "{:?}", names(videos.path()));
        assert!(names(root.path()).is_empty(), "pass-log folder left behind after cancel");

        // An .mkv isn't Discord-ready, so the copy is encoded, as passes 2 and 3 of 3.
        let dir = tempfile::tempdir().unwrap();
        let input = small_clip(&paths, dir.path());
        let mut passes = Vec::new();
        let mut saw_log = false;
        let mut record = |p: JobProgress| {
            passes.push((p.pass, p.passes));
            saw_log |= p.pass == 3 && !names(root.path()).is_empty();
        };
        let outcome = export_work(&paths, &fit_request(&input), &lookup, &CancelToken::new(), &mut record, root.path());
        let Ok(JobOutcome::Done { output, note: None, .. }) = outcome else { panic!("{outcome:?}") };
        assert_eq!(output, dir.path().join("[CLIPPED] clip.mkv"));
        assert_eq!(names(videos.path()), ["[20MB] clip.mp4"]);
        assert!(saw_log, "the pass log should live under the work root");
        assert!(names(root.path()).is_empty(), "pass-log folder left behind after success");
        for pass in 1..=3 {
            assert!(passes.contains(&(pass, 3)), "no pass {pass} of 3 in {passes:?}");
        }
    }

    #[test]
    fn a_missing_videos_folder_is_a_note() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = small_clip(&paths, dir.path());
        let lookup = || Err(FsError::new(NO_VIDEOS_FOLDER));

        let outcome = export_work(&paths, &fit_request(&input), &lookup, &CancelToken::new(), &mut |_| {}, dir.path());
        let Ok(JobOutcome::Done { output, note, .. }) = outcome else { panic!("{outcome:?}") };
        assert_eq!(output, dir.path().join("[CLIPPED] clip.mkv"));
        assert_eq!(
            note.as_deref(),
            Some("Saved \"[CLIPPED] clip.mkv\", but couldn't make the 20 MB copy: Couldn't find your Videos folder.")
        );
    }

    #[test]
    fn video_export_without_video_is_refused() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = small_clip(&paths, dir.path());
        for (mode, fit) in [(ExportMode::Quick, false), (ExportMode::SaveRaw, true)] {
            let req = ExportRequest { mode, fit_20mb: fit, video: Vec::new(), ..fit_request(&input) };
            let mut ran = false;
            let lookup = || -> Result<PathBuf, FsError> { panic!("no copy without an export") };
            let err = export_work(&paths, &req, &lookup, &CancelToken::new(), &mut |_| ran = true, dir.path())
                .unwrap_err();
            assert_eq!(err.to_string(), NO_VIDEO);
            assert!(!ran, "ffmpeg must not run");
        }
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["clip.mkv"]);
    }
}
