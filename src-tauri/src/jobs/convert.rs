//! Convert plan and job (Task 8).
//!
//! Converting replaces a file with the target format: remuxing (`-c copy`) when the codecs fit
//! the target container, re-encoding otherwise. The original goes to the Recycle Bin only after
//! the new file has been verified.

use std::ffi::OsString;
use std::path::Path;

use crate::cli::Target;
use crate::fsops::{commit_replace, temp_path_for, unique_path, with_extension};
use crate::media::run::{run_ffmpeg, CancelToken};
use crate::media::{probe, FfmpegPaths, MediaInfo};
use crate::verify::verify_output;

use super::{JobError, JobOutcome, JobProgress};

/// How a file is converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvertPlan {
    /// Copy every kept stream into the new container.
    Remux,
    /// Re-encode video to H.264 and every audio stream to AAC.
    ReencodeVideo,
    /// Mix all audio streams into one MP3 and drop the video.
    ExtractMp3,
}

const MP4_VIDEO: &[&str] = &["h264", "hevc", "av1"];
const MP4_AUDIO: &[&str] = &["aac", "mp3", "opus", "ac3"];
const MOV_VIDEO: &[&str] = &["h264", "hevc", "prores"];
const MOV_AUDIO: &[&str] = &["aac", "pcm_s16le", "pcm_s24le"];

fn extension(target: &Target) -> &'static str {
    match target {
        Target::Mp4 => "mp4",
        Target::Mov => "mov",
        Target::Mp3 => "mp3",
    }
}

/// Decide how to convert `info` to `target`.
pub fn plan_convert(info: &MediaInfo, target: Target) -> Result<ConvertPlan, JobError> {
    let ext = extension(&target);
    if info.container.eq_ignore_ascii_case(ext) {
        return Err(JobError::Message(format!("Already a .{ext} file")));
    }
    let (video_ok, audio_ok) = match target {
        Target::Mp3 => {
            return if info.audio.is_empty() {
                Err(JobError::Message("This video has no audio".to_string()))
            } else {
                Ok(ConvertPlan::ExtractMp3)
            };
        }
        Target::Mp4 => (MP4_VIDEO, MP4_AUDIO),
        Target::Mov => (MOV_VIDEO, MOV_AUDIO),
    };
    let video_fits = match &info.video {
        Some(v) => video_ok.contains(&v.codec.as_str()),
        // Mp4 accepts audio-only input as is; for Mov the plan requires a listed video codec.
        None => matches!(target, Target::Mp4),
    };
    let audio_fits = info.audio.iter().all(|a| audio_ok.contains(&a.codec.as_str()));
    Ok(if video_fits && audio_fits { ConvertPlan::Remux } else { ConvertPlan::ReencodeVideo })
}

fn os(s: &str) -> OsString {
    OsString::from(s)
}

/// ffmpeg arguments (after the runner's own flags) writing `info`'s file to `temp`.
fn build_args(info: &MediaInfo, input: &Path, temp: &Path, target: &Target, plan: ConvertPlan) -> Vec<OsString> {
    let mut args = vec![os("-i"), input.as_os_str().to_os_string()];
    let video_map = info.video.as_ref().map(|v| format!("0:{}", v.index));
    match plan {
        ConvertPlan::ExtractMp3 => {
            let n = info.audio.len();
            if n == 1 {
                args.extend([os("-map"), os(&format!("0:{}", info.audio[0].index))]);
            } else {
                let inputs: String = (0..n).map(|i| format!("[0:a:{i}]")).collect();
                args.extend([
                    os("-filter_complex"),
                    os(&format!("{inputs}amix=inputs={n}:normalize=0[a]")),
                    os("-map"),
                    os("[a]"),
                ]);
            }
            args.extend([os("-vn"), os("-c:a"), os("libmp3lame"), os("-q:a"), os("2")]);
        }
        ConvertPlan::Remux | ConvertPlan::ReencodeVideo => {
            // Map the real video stream explicitly so cover art is never carried along.
            if let Some(map) = &video_map {
                args.extend([os("-map"), os(map)]);
            }
            args.extend([os("-map"), os("0:a?")]);
            if plan == ConvertPlan::Remux {
                args.extend([os("-c"), os("copy")]);
                let hevc = info.video.as_ref().is_some_and(|v| v.codec == "hevc");
                if hevc && matches!(target, Target::Mov) {
                    // QuickTime and Windows only recognise HEVC tagged hvc1.
                    args.extend([os("-tag:v"), os("hvc1")]);
                }
            } else {
                args.extend([
                    os("-c:v"), os("libx264"), os("-crf"), os("18"), os("-preset"), os("medium"),
                    os("-pix_fmt"), os("yuv420p"),
                    os("-c:a"), os("aac"), os("-b:a"), os("192k"),
                ]);
            }
        }
    }
    // The temp file's extension is `.hasami-tmp`, so the muxer can't be inferred from the name.
    args.extend([os("-f"), os(extension(target)), temp.as_os_str().to_os_string()]);
    args
}

/// Convert `input` to `target`, replacing it once the new file is verified.
pub fn convert(
    paths: &FfmpegPaths,
    input: &Path,
    target: Target,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
) -> Result<JobOutcome, JobError> {
    let info = probe(paths, input)?;
    let plan = plan_convert(&info, target.clone())?;

    let final_path = unique_path(&with_extension(input, extension(&target)), Some(input));
    let temp = temp_path_for(&final_path);

    let result = encode_and_commit(paths, input, &info, &target, plan, &final_path, &temp, cancel, on_progress);
    if result.as_ref().is_err_and(|e| !e.keeps_new_file()) {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn encode_and_commit(
    paths: &FfmpegPaths,
    input: &Path,
    info: &MediaInfo,
    target: &Target,
    plan: ConvertPlan,
    final_path: &Path,
    temp: &Path,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
) -> Result<JobOutcome, JobError> {
    let args = build_args(info, input, temp, target, plan);
    run_ffmpeg(paths, &args, info.duration_s, cancel, &mut |fraction| {
        on_progress(JobProgress { pass: 1, passes: 1, fraction })
    })?;
    verify_output(paths, temp, info.duration_s, None)?;
    // Last chance to back out before the original is touched.
    if cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    commit_replace(temp, input, final_path)?;
    let bytes = std::fs::metadata(final_path).map(|m| m.len()).unwrap_or(0);
    Ok(JobOutcome::Done { output: final_path.to_path_buf(), bytes })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::media::{AudioStream, MediaInfo, VideoStream};

    fn info(container: &str, video: Option<&str>, audio: &[&str]) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from(format!("clip.{container}")),
            container: container.to_string(),
            duration_s: 10.0,
            size_bytes: 1_000_000,
            bit_rate: None,
            video: video.map(|codec| VideoStream {
                index: 0,
                codec: codec.to_string(),
                width: 1920,
                height: 1080,
                fps: 30.0,
                bit_rate: None,
            }),
            audio: audio
                .iter()
                .enumerate()
                .map(|(i, codec)| AudioStream {
                    index: i + 1,
                    codec: codec.to_string(),
                    channels: 2,
                    sample_rate: 48000,
                    bit_rate: None,
                })
                .collect(),
        }
    }

    #[test]
    fn plan_hevc_mp4_to_mov_remux() {
        let plan = plan_convert(&info("mp4", Some("hevc"), &["aac", "aac", "aac"]), Target::Mov).unwrap();
        assert_eq!(plan, ConvertPlan::Remux);
    }

    #[test]
    fn plan_vp9_webm_to_mp4_reencode() {
        let plan = plan_convert(&info("webm", Some("vp9"), &["opus"]), Target::Mp4).unwrap();
        assert_eq!(plan, ConvertPlan::ReencodeVideo);
    }

    #[test]
    fn plan_mp3_no_audio_errors() {
        let err = plan_convert(&info("mp4", Some("h264"), &[]), Target::Mp3).unwrap_err();
        assert_eq!(err.to_string(), "This video has no audio");
    }

    #[test]
    fn plan_mp3_with_audio_extracts() {
        let plan = plan_convert(&info("mp4", Some("h264"), &["aac"]), Target::Mp3).unwrap();
        assert_eq!(plan, ConvertPlan::ExtractMp3);
    }

    #[test]
    fn plan_mp4_remuxes_allowed_codecs_and_audio_only() {
        assert_eq!(
            plan_convert(&info("mov", Some("h264"), &["aac", "ac3"]), Target::Mp4).unwrap(),
            ConvertPlan::Remux
        );
        assert_eq!(plan_convert(&info("mkv", None, &["opus"]), Target::Mp4).unwrap(), ConvertPlan::Remux);
    }

    #[test]
    fn plan_mp4_reencodes_when_any_audio_stream_is_incompatible() {
        let plan = plan_convert(&info("mkv", Some("h264"), &["aac", "flac"]), Target::Mp4).unwrap();
        assert_eq!(plan, ConvertPlan::ReencodeVideo);
    }

    #[test]
    fn plan_mov_rules() {
        assert_eq!(
            plan_convert(&info("mp4", Some("h264"), &["aac"]), Target::Mov).unwrap(),
            ConvertPlan::Remux
        );
        assert_eq!(
            plan_convert(&info("mkv", Some("prores"), &["pcm_s24le"]), Target::Mov).unwrap(),
            ConvertPlan::Remux
        );
        // av1 is fine in mp4 but not in mov; opus/mp3 audio likewise.
        assert_eq!(
            plan_convert(&info("mp4", Some("av1"), &["aac"]), Target::Mov).unwrap(),
            ConvertPlan::ReencodeVideo
        );
        assert_eq!(
            plan_convert(&info("mp4", Some("h264"), &["mp3"]), Target::Mov).unwrap(),
            ConvertPlan::ReencodeVideo
        );
    }

    #[test]
    fn plan_same_extension_errors() {
        let err = plan_convert(&info("mp4", Some("h264"), &["aac"]), Target::Mp4).unwrap_err();
        assert_eq!(err.to_string(), "Already a .mp4 file");
        let err = plan_convert(&info("mp3", None, &["mp3"]), Target::Mp3).unwrap_err();
        assert_eq!(err.to_string(), "Already a .mp3 file");
    }
}
