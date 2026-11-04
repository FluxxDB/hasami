//! Export job: render the Quick Trim editor's edit and save it beside the source as
//! `[CLIPPED] <name>`.
//!
//! - **Quick export** and **Export + save raw** re-encode the edit as Discord-safe H.264 with AAC
//!   in `.mp4`, whatever the source was, at about the source's bitrate (more for HEVC, AV1 and
//!   VP9, see [`export_bitrate`]). With a crop, the picture is cut to it and scaled back up to the
//!   source's size. The original then goes to the Recycle Bin, or into a `Raw` folder next to it.
//! - With the **20 MB copy** toggle on, they then also save `[20MB] <name>.mp4` (at most
//!   [`SIZE_LIMIT_BYTES`]) in the Windows Videos folder, made from the saved `[CLIPPED]` file by
//!   the Compress pipeline, or copied as is when that file is already small and Discord-ready.
//!   A failed copy never fails the export: the outcome is still `Done`, with a note saying why.
//! - **Export audio** mixes the kept audio into one `.mp3`; the original is untouched.
//!
//! A video export uses AMD's hardware H.264 encoder when it works here and the picture isn't too
//! big for it; if a hardware render fails anyway, it is done once more with libx264.
//!
//! As with every job, each result is written to a hidden temp file, verified, and only then given
//! its name; the original is touched last. On any error or cancel the temp file is deleted.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use super::compress::encode_to_limit;
use super::output::{claim_new_file, commit_claimed, RemoveOnDrop, VideosDirFn};
use super::{JobError, JobOutcome, JobProgress, UNREADABLE};
use crate::compress::{plan_compress, CompressDecision, SIZE_LIMIT_BYTES};
use crate::editor::media::Crop;
use crate::editor::request::{
    build_graph, encoder_available, export_bitrate, h264_encoder, output_size, software_h264, ExportMode,
    ExportRequest, Graph, VideoEncoder,
};
use crate::fsops::{clipped_path, commit_new, commit_replace, compressed_path, move_to_raw, temp_path_for, videos_dir};
use crate::media::run::{run_ffmpeg, CancelToken};
use crate::media::{probe, FfmpegPaths, MediaInfo};
use crate::verify::{verify_output, VerifyError};

/// Shown for an audio export when every audio track is muted or the source has none.
const NO_AUDIO: &str = "This clip has no audio to export.";
/// Shown for a video export whose timeline has no video left in it.
const NO_VIDEO: &str = "Couldn't export this clip: there's no video left in it.";
/// Shown for a video export of a source that has no video stream at all.
const SOURCE_HAS_NO_VIDEO: &str = "This clip has no video to export.";
/// Shown when the area to keep doesn't fit inside the source's picture.
const CROP_DOESNT_FIT: &str = "Couldn't export this clip: the border to remove doesn't fit the picture.";
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

/// Picks the video encoder for an export at a bitrate (bits/s) and output size (width, height).
type EncoderFn<'a> = &'a dyn Fn(u64, (u32, u32)) -> VideoEncoder;

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
    // H.264 on AMD hardware when it works here and the picture isn't too big for it.
    let pick = |bitrate, size| h264_encoder(bitrate, size, &|name| encoder_available(paths, name));
    export_with(paths, req, &pick, videos_dir, cancel, on_progress, work_root)
}

/// [`export_work`] with the encoder choice given, so tests can make the first render fail.
fn export_with(
    paths: &FfmpegPaths,
    req: &ExportRequest,
    pick_encoder: EncoderFn,
    videos_dir: VideosDirFn,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
    work_root: &Path,
) -> Result<JobOutcome, JobError> {
    let info = probe(paths, &req.input).map_err(|_| JobError::Message(UNREADABLE.to_owned()))?;

    let source_size = info.video.as_ref().map_or((0, 0), |v| (v.width, v.height));

    let (render, graph) = if req.mode == ExportMode::Audio {
        // Video pieces would leave `[vout]` unconnected; only the audio timeline is wanted.
        let graph = build_graph(&ExportRequest { video: Vec::new(), ..req.clone() }, source_size);
        if graph.audio_out.is_none() {
            return Err(JobError::Message(NO_AUDIO.to_owned()));
        }
        (Render::Mp3, graph)
    } else if req.video.is_empty() {
        return Err(JobError::Message(NO_VIDEO.to_owned()));
    } else if info.video.is_none() {
        return Err(JobError::Message(SOURCE_HAS_NO_VIDEO.to_owned()));
    } else if req.crop.is_some_and(|c| !fits(c, source_size)) {
        return Err(JobError::Message(CROP_DOESNT_FIT.to_owned()));
    } else {
        (Render::H264, build_graph(req, source_size))
    };
    // The toggle belongs to the two video exports.
    let make_copy = req.fit_20mb && render != Render::Mp3;
    let passes = if make_copy { COPY_PASSES } else { 1 };

    let final_path = clipped_path(&req.input, render.extension());
    // Deleted however this ends (a committed temp is already gone), unless a failed commit
    // left the only copy of the result there.
    let mut temp = RemoveOnDrop::file(temp_path_for(&final_path));
    let job = Job { paths, req, info: &info, graph: &graph, render, temp: temp.path(), passes, pick_encoder };
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
    let info = probe(paths, clipped).map_err(|_| JobError::Message(UNREADABLE.to_owned()))?;
    let final_path = compressed_path(clipped, &videos_dir()?);
    let mut temp = claim_new_file(&final_path)?;

    match plan_compress(&info)? {
        CompressDecision::AlreadySmall => {
            fs::copy(clipped, temp.path()).map_err(|_| JobError::Message(COPY_FAILED.to_owned()))?;
            verify_output(paths, temp.path(), info.duration_s, Some(SIZE_LIMIT_BYTES)).map_err(plain_verify_error)?;
            on_progress(JobProgress { pass: 2, passes: 2, fraction: 1.0 });
        }
        CompressDecision::Encode(plan) => {
            encode_to_limit(paths, clipped, &info, &plan, temp.path(), work_root, cancel, on_progress)?;
        }
    }

    commit_claimed(&mut temp, &final_path, cancel)?;
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

/// Whether `crop` is a non-empty area inside a `size` (width, height) picture.
fn fits(crop: Crop, (width, height): (u32, u32)) -> bool {
    let Crop { x, y, w, h } = crop;
    let within = |start: u32, len: u32, limit: u32| u64::from(start) + u64::from(len) <= u64::from(limit);
    w > 0 && h > 0 && within(x, w, width) && within(y, h, height)
}

/// How the edit is turned into a file.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Render {
    /// H.264 video and AAC audio in `.mp4`.
    H264,
    /// The audio mix alone, as MP3.
    Mp3,
}

impl Render {
    fn extension(self) -> &'static str {
        match self {
            Render::H264 => "mp4",
            Render::Mp3 => "mp3",
        }
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
    pick_encoder: EncoderFn<'a>,
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
        let size = match self.render {
            Render::Mp3 => self.attempt(&mp3_args(input, self.graph, self.temp), cancel, on_progress)?,
            Render::H264 => {
                let bitrate = export_bitrate(self.info);
                let source_size = self.info.video.as_ref().map_or((0, 0), |v| (v.width, v.height));
                let first = (self.pick_encoder)(bitrate, output_size(self.req.crop, source_size));
                let software = software_h264(bitrate);
                match self.attempt(&h264_args(input, self.graph, &first, self.temp), cancel, on_progress) {
                    // Nothing is committed yet: a failed hardware render is done once more in
                    // software, from a clean temp file, unless the user cancelled meanwhile.
                    Err(e)
                        if first.codec != software.codec
                            && !matches!(e, JobError::Cancelled)
                            && !cancel.is_cancelled() =>
                    {
                        eprintln!("hasami: {} export failed, retrying with {}: {e}", first.codec, software.codec);
                        let _ = fs::remove_file(self.temp);
                        self.attempt(&h264_args(input, self.graph, &software, self.temp), cancel, on_progress)?
                    }
                    other => other?,
                }
            }
        };

        // Last chance to back out; once the commit starts it runs to completion or rolls back.
        if cancel.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        match self.req.mode {
            ExportMode::Quick => commit_replace(self.temp, input, final_path)?,
            ExportMode::SaveRaw => {
                commit_new(self.temp, final_path)?;
                // The new file is saved either way; only the original's move can still fail.
                move_to_raw(input).map_err(|_| JobError::Message(raw_move_failed(final_path, self.req.fit_20mb)))?;
            }
            ExportMode::Audio => commit_new(self.temp, final_path)?,
        }
        Ok(size)
    }

    /// Run ffmpeg with `args` into the temp file (as pass 1) and verify the result. Returns its size.
    fn attempt(
        &self,
        args: &[OsString],
        cancel: &CancelToken,
        on_progress: &mut dyn FnMut(JobProgress),
    ) -> Result<u64, JobError> {
        run_ffmpeg(self.paths, args, self.req.duration_s, cancel, &mut |fraction| {
            on_progress(JobProgress { pass: 1, passes: self.passes, fraction })
        })?;
        let verified = verify_output(self.paths, self.temp, self.req.duration_s, None).map_err(plain_verify_error)?;
        Ok(verified.size_bytes)
    }
}

/// `Saved "<name>", but couldn't move the original into the Raw folder.`, plus that the 20 MB
/// copy wasn't made when one was asked for (it is only made once the original is dealt with).
fn raw_move_failed(final_path: &Path, copy_asked: bool) -> String {
    let name = final_path.file_name().unwrap_or_default().to_string_lossy();
    let copy = if copy_asked { " The 20 MB copy wasn't made." } else { "" };
    format!("Saved \"{name}\", but couldn't move the original into the Raw folder.{copy}")
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

/// Quick / Save raw: H.264 with AAC in MP4, its index up front so the clip starts playing before
/// it fully loads. The muxer is named since the temp file's `.hasami-tmp` extension says nothing.
fn h264_args(input: &Path, graph: &Graph, encoder: &VideoEncoder, out: &Path) -> Vec<OsString> {
    let mut a = graph_args(input, graph);
    push(&mut a, &["-c:v", encoder.codec]);
    a.extend(encoder.args.iter().map(OsString::from));
    if graph.audio_out.is_some() {
        push(&mut a, &["-c:a", "aac", "-b:a", "192k"]);
    }
    push(&mut a, &["-movflags", "+faststart", "-f", "mp4"]);
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
    use crate::fsops::{FsError, NO_VIDEOS_FOLDER};
    use crate::jobs::test_clips;

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

    fn encoder() -> VideoEncoder {
        VideoEncoder { codec: "h264_amf", args: vec!["-b:v".to_owned(), "9000000".to_owned()] }
    }

    fn h264(audio: bool) -> String {
        text(&h264_args(Path::new("in.mkv"), &graph(audio), &encoder(), Path::new("tmp")))
    }

    #[test]
    fn h264_maps_the_graph_into_an_mp4() {
        assert_eq!(
            h264(true),
            "-i in.mkv -filter_complex G -map [vout] -map [aout] -c:v h264_amf -b:v 9000000 \
             -c:a aac -b:a 192k -movflags +faststart -f mp4 tmp"
        );
    }

    #[test]
    fn h264_without_audio_drops_it() {
        let a = h264(false);
        assert!(a.contains("-map [vout] -an "), "{a}");
        assert!(!a.contains("-c:a"), "{a}");
        assert!(a.ends_with("-movflags +faststart -f mp4 tmp"), "{a}");
    }

    #[test]
    fn a_crop_must_fit_the_picture() {
        let size = (1920, 1080);
        assert!(fits(Crop { x: 0, y: 0, w: 1440, h: 810 }, size));
        assert!(fits(Crop { x: 480, y: 270, w: 1440, h: 810 }, size));
        assert!(!fits(Crop { x: 481, y: 0, w: 1440, h: 810 }, size));
        assert!(!fits(Crop { x: 0, y: 271, w: 1440, h: 810 }, size));
        assert!(!fits(Crop { x: 0, y: 0, w: 0, h: 810 }, size));
        assert!(!fits(Crop { x: u32::MAX, y: 0, w: 2, h: 2 }, size));
        assert!(!fits(Crop { x: 0, y: 0, w: 2, h: 2 }, (0, 0)), "no video");
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
            raw_move_failed(Path::new(r"C:\d\[CLIPPED] clip.mp4"), false),
            "Saved \"[CLIPPED] clip.mp4\", but couldn't move the original into the Raw folder."
        );
        assert_eq!(
            raw_move_failed(Path::new(r"C:\d\[CLIPPED] clip.mp4"), true),
            "Saved \"[CLIPPED] clip.mp4\", but couldn't move the original into the Raw folder. \
             The 20 MB copy wasn't made."
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
            crop: None,
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
        let input = test_clips::small(&paths, dir.path(), "clip.mkv", true);
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
        let input = test_clips::small(&paths, dir.path(), "clip.mkv", true);
        let cancel = CancelToken::new();
        let mut cancel_in_copy = |p: JobProgress| {
            if p.pass >= 2 {
                cancel.cancel();
            }
        };
        let outcome = export_work(&paths, &fit_request(&input), &lookup, &cancel, &mut cancel_in_copy, root.path());
        let Ok(JobOutcome::Done { output, note, .. }) = outcome else { panic!("{outcome:?}") };
        assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
        assert_eq!(
            note.as_deref(),
            Some("Saved \"[CLIPPED] clip.mp4\", but couldn't make the 20 MB copy: it was cancelled.")
        );
        assert_eq!(names(dir.path()), ["Raw", "[CLIPPED] clip.mp4"]);
        assert!(names(videos.path()).is_empty(), "{:?}", names(videos.path()));
        assert!(names(root.path()).is_empty(), "pass-log folder left behind after cancel");

        // An export over 20 MB isn't Discord-ready, so the copy is encoded, as passes 2 and 3 of 3.
        let dir = tempfile::tempdir().unwrap();
        let input = test_clips::big(&paths, dir.path(), "clip.mkv");
        let whole = ExportRequest {
            duration_s: 4.0,
            video: vec![VideoPiece { src_start: 0.0, src_end: 4.0 }],
            audio: Vec::new(),
            ..fit_request(&input)
        };
        let mut passes = Vec::new();
        let mut saw_log = false;
        let mut record = |p: JobProgress| {
            passes.push((p.pass, p.passes));
            saw_log |= p.pass == 3 && !names(root.path()).is_empty();
        };
        let outcome = export_work(&paths, &whole, &lookup, &CancelToken::new(), &mut record, root.path());
        let Ok(JobOutcome::Done { output, note: None, .. }) = outcome else { panic!("{outcome:?}") };
        assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
        assert!(fs::metadata(&output).unwrap().len() > SIZE_LIMIT_BYTES, "the export should be over 20 MB");
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
        let input = test_clips::small(&paths, dir.path(), "clip.mkv", true);
        let lookup = || Err(FsError::new(NO_VIDEOS_FOLDER));

        let outcome = export_work(&paths, &fit_request(&input), &lookup, &CancelToken::new(), &mut |_| {}, dir.path());
        let Ok(JobOutcome::Done { output, note, .. }) = outcome else { panic!("{outcome:?}") };
        assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
        assert_eq!(
            note.as_deref(),
            Some("Saved \"[CLIPPED] clip.mp4\", but couldn't make the 20 MB copy: Couldn't find your Videos folder.")
        );
    }

    #[test]
    fn an_audio_export_ignores_the_20mb_copy_toggle() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = test_clips::small(&paths, dir.path(), "clip.mkv", true);
        let req = ExportRequest { mode: ExportMode::Audio, ..fit_request(&input) };
        let lookup = || -> Result<PathBuf, FsError> { panic!("an audio export makes no 20 MB copy") };

        let mut passes = Vec::new();
        let outcome =
            export_work(&paths, &req, &lookup, &CancelToken::new(), &mut |p| passes.push(p.passes), dir.path());
        let Ok(JobOutcome::Done { output, note: None, .. }) = outcome else { panic!("{outcome:?}") };
        assert_eq!(output, dir.path().join("[CLIPPED] clip.mp3"));
        assert!(passes.iter().all(|&of| of == 1), "{passes:?}");
        assert_eq!(names(dir.path()), ["[CLIPPED] clip.mp3", "clip.mkv"]);
    }

    /// Whether `file` was encoded by libx264, which signs its stream.
    fn made_by_x264(file: &Path) -> bool {
        fs::read(file).unwrap().windows(11).any(|w| w == b"x264 - core")
    }

    #[test]
    fn a_failed_hardware_render_is_done_again_in_software() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = test_clips::small(&paths, dir.path(), "clip.mkv", true);
        let req = ExportRequest { fit_20mb: false, ..fit_request(&input) };
        let lookup = || -> Result<PathBuf, FsError> { panic!("no 20 MB copy was asked for") };
        // A hardware encoder this ffmpeg doesn't have fails the first render outright.
        let broken = |_: u64, _: (u32, u32)| VideoEncoder { codec: "hasami_no_such_encoder", args: Vec::new() };

        let outcome = export_with(&paths, &req, &broken, &lookup, &CancelToken::new(), &mut |_| {}, dir.path());
        let Ok(JobOutcome::Done { output, note: None, .. }) = outcome else { panic!("{outcome:?}") };
        assert_eq!(output, dir.path().join("[CLIPPED] clip.mp4"));
        assert!(made_by_x264(&output), "the retry should be libx264");
        let info = probe(&paths, &output).unwrap();
        assert_eq!(info.video.as_ref().unwrap().codec, "h264");
        assert!((info.duration_s - 1.5).abs() < 0.1, "duration was {}", info.duration_s);
        assert_eq!(names(dir.path()), ["Raw", "[CLIPPED] clip.mp4"], "no temp file left behind");
    }

    #[test]
    fn a_failed_software_render_is_not_retried() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = test_clips::small(&paths, dir.path(), "clip.mkv", true);
        let req = ExportRequest { fit_20mb: false, ..fit_request(&input) };
        let lookup = || -> Result<PathBuf, FsError> { panic!("no 20 MB copy was asked for") };
        // libx264 with a bad preset fails; a retry, with the usual libx264 settings, would succeed.
        let broken = |_: u64, _: (u32, u32)| VideoEncoder {
            codec: "libx264",
            args: vec!["-preset".to_owned(), "no_such_preset".to_owned()],
        };

        let outcome = export_with(&paths, &req, &broken, &lookup, &CancelToken::new(), &mut |_| {}, dir.path());
        assert!(matches!(outcome, Err(JobError::Encode { .. })), "{outcome:?}");
        assert_eq!(names(dir.path()), ["clip.mkv"], "only the untouched input may remain");
    }

    #[test]
    fn video_export_without_video_is_refused() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = test_clips::small(&paths, dir.path(), "clip.mkv", true);
        for (mode, fit) in [(ExportMode::Quick, false), (ExportMode::SaveRaw, true)] {
            let req = ExportRequest { mode, fit_20mb: fit, video: Vec::new(), ..fit_request(&input) };
            let mut ran = false;
            let lookup = || -> Result<PathBuf, FsError> { panic!("no copy without an export") };
            let err =
                export_work(&paths, &req, &lookup, &CancelToken::new(), &mut |_| ran = true, dir.path()).unwrap_err();
            assert_eq!(err.to_string(), NO_VIDEO);
            assert!(!ran, "ffmpeg must not run");
        }
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["clip.mkv"]);
    }

    #[test]
    fn a_video_export_of_a_source_without_video_is_refused() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = test_clips::audio_only(&paths, dir.path(), "sound.mkv");
        let req = ExportRequest { input: input.clone(), fit_20mb: false, ..fit_request(&input) };
        let lookup = || -> Result<PathBuf, FsError> { panic!("no copy without an export") };
        let mut ran = false;
        let err = export_work(&paths, &req, &lookup, &CancelToken::new(), &mut |_| ran = true, dir.path()).unwrap_err();
        assert_eq!(err.to_string(), SOURCE_HAS_NO_VIDEO);
        assert!(!ran, "ffmpeg must not run");
        assert_eq!(names(dir.path()), ["sound.mkv"]);
    }

    #[test]
    fn a_crop_outside_the_picture_is_refused() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = test_clips::small(&paths, dir.path(), "clip.mkv", true);
        // The clip is 320x180.
        let req = ExportRequest { crop: Some(Crop { x: 0, y: 0, w: 320, h: 182 }), ..fit_request(&input) };
        let lookup = || -> Result<PathBuf, FsError> { panic!("no copy without an export") };
        let mut ran = false;
        let err = export_work(&paths, &req, &lookup, &CancelToken::new(), &mut |_| ran = true, dir.path()).unwrap_err();
        assert_eq!(err.to_string(), CROP_DOESNT_FIT);
        assert!(!ran, "ffmpeg must not run");
        assert_eq!(names(dir.path()), ["clip.mkv"]);
    }
}
