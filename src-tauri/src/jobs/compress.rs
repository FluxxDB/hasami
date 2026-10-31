//! Compress job: re-encode any input into an H.264/AAC `.mp4` of at most
//! [`SIZE_LIMIT_BYTES`] and replace the original safely.
//!
//! The encode goes to a hidden temp file next to the output, is verified (duration and size),
//! and only then replaces the original, which goes to the Recycle Bin. On any error or cancel
//! the temp file and the two-pass logs are deleted and the original is left as it was.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{JobError, JobOutcome, JobProgress, UNREADABLE};
use crate::compress::{plan_compress, CompressDecision, CompressPlan, SIZE_LIMIT_BYTES};
use crate::fsops::{commit_replace, temp_path_for, unique_path, with_extension};
use crate::media::run::{run_ffmpeg, CancelToken};
use crate::media::{probe, FfmpegPaths, MediaInfo};
use crate::verify::{verify_output, VerifyError};

/// Reason given when the input is left alone because it already meets the limit.
const ALREADY_SMALL: &str = "Already under 20 MB";
/// Re-encodes allowed after the first one comes out over the limit.
const MAX_RETRIES: u32 = 3;
/// Each retry multiplies the bitrate by this.
const RETRY_FACTOR: f64 = 0.9;
/// Shown when every retry was still over the limit.
const NEVER_FIT: &str = "Couldn't get this video under 20 MB.";
/// Shown when an encoded result fails verification for a reason other than its size.
const DIDNT_CHECK_OUT: &str = "Couldn't compress this video: the result didn't check out.";
/// Where pass 1 writes its (discarded) output. The crate is Windows-only.
const NULL_OUTPUT: &str = "NUL";

/// Compress `input` to an `.mp4` of at most [`SIZE_LIMIT_BYTES`] and replace it.
///
/// - Already small and compatible: [`JobOutcome::Unchanged`], nothing is touched.
/// - The plan drops the video below 480p (and below the source's own height) and
///   `confirmed_low_res` is false: [`JobOutcome::NeedsConfirm`], nothing is written.
/// - Otherwise a two-pass libx264 encode (one AAC pass for audio-only input), retried at a
///   lower bitrate up to [`MAX_RETRIES`] times if it comes out too big. The output keeps the
///   input's name with an `.mp4` extension, numbered if another file already has that name.
pub fn compress(
    paths: &FfmpegPaths,
    input: &Path,
    confirmed_low_res: bool,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
) -> Result<JobOutcome, JobError> {
    compress_in(
        paths,
        input,
        confirmed_low_res,
        cancel,
        on_progress,
        &std::env::temp_dir(),
    )
}

/// [`compress`] with the parent folder of the two-pass log folder given, so tests can check
/// that it is cleaned up.
fn compress_in(
    paths: &FfmpegPaths,
    input: &Path,
    confirmed_low_res: bool,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
    work_root: &Path,
) -> Result<JobOutcome, JobError> {
    let info = probe(paths, input).map_err(|_| JobError::Message(UNREADABLE.to_owned()))?;
    let plan = match plan_compress(&info)? {
        CompressDecision::AlreadySmall => {
            return Ok(JobOutcome::Unchanged {
                reason: ALREADY_SMALL.to_owned(),
            })
        }
        CompressDecision::Encode(plan) => plan,
    };

    // A source that is already below 480p isn't "falling" below it, so only ask when the
    // plan actually shrinks the picture under the threshold.
    let source_height = info.video.as_ref().map_or(0, |v| v.height);
    if plan.warn_low_res && plan.height < source_height && !confirmed_low_res {
        return Ok(JobOutcome::NeedsConfirm { plan });
    }

    let final_path = unique_path(&desired_output(input), Some(input));
    let mut temp = RemoveOnDrop::file(temp_path_for(&final_path));
    let verified = encode_to_limit(
        paths,
        input,
        &info,
        &plan,
        temp.path(),
        work_root,
        cancel,
        on_progress,
    )?;

    // Last chance to back out; once the commit starts it runs to completion or rolls back.
    if cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    if let Err(e) = commit_replace(temp.path(), input, &final_path) {
        if e.kept_new_file.is_some() {
            // The original is already in the Recycle Bin; this is the only copy of the result.
            temp.keep();
        }
        return Err(e.into());
    }
    Ok(JobOutcome::Done {
        output: final_path,
        bytes: verified.size_bytes,
    })
}

/// Two-pass encode of `input` per `plan` into `output`, retried smaller until it verifies at
/// ≤ [`SIZE_LIMIT_BYTES`]. Creates and removes its own pass-log folder under `work_root`.
/// Errors are already plain-language (`UNREADABLE` / `DIDNT_CHECK_OUT` / `NEVER_FIT` / encode).
#[allow(clippy::too_many_arguments)] // the shape the editor export is specified against
pub(crate) fn encode_to_limit(
    paths: &FfmpegPaths,
    input: &Path,
    info: &MediaInfo,
    plan: &CompressPlan,
    output: &Path,
    work_root: &Path,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(JobProgress),
) -> Result<MediaInfo, JobError> {
    let work = RemoveOnDrop::dir(create_work_dir(work_root)?);
    let passlog = work.path().join("x264");

    encode_until_fits(|scale| {
        let job = Encode {
            input,
            info,
            plan,
            scale,
            passlog: &passlog,
            output,
        };
        job.run(paths, cancel, on_progress)?;
        Ok(verify_output(
            paths,
            output,
            info.duration_s,
            Some(SIZE_LIMIT_BYTES),
        )?)
    })
    .map_err(plain_verify_error)
}

/// The output name before de-duplication: the input itself if it already is an `.mp4`
/// (keeping its exact spelling, e.g. `.MP4`), otherwise the input with an `.mp4` extension.
fn desired_output(input: &Path) -> PathBuf {
    let is_mp4 = input
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
    if is_mp4 {
        input.to_path_buf()
    } else {
        with_extension(input, "mp4")
    }
}

/// Call `attempt` with a bitrate scale of 1.0, then, while the result is over the size limit,
/// up to [`MAX_RETRIES`] more times with the scale multiplied by [`RETRY_FACTOR`] each time.
/// Any other error is returned at once.
fn encode_until_fits<T>(
    mut attempt: impl FnMut(f64) -> Result<T, JobError>,
) -> Result<T, JobError> {
    let mut scale = 1.0;
    let mut retries = 0;
    loop {
        match attempt(scale) {
            Err(JobError::Verify(VerifyError::TooLarge { .. })) if retries < MAX_RETRIES => {
                retries += 1;
                scale *= RETRY_FACTOR;
            }
            Err(JobError::Verify(VerifyError::TooLarge { .. })) => {
                return Err(JobError::Message(NEVER_FIT.to_owned()))
            }
            other => return other,
        }
    }
}

/// Replace a verification failure's technical text with plain language for the user.
/// (`TooLarge` never gets here: [`encode_until_fits`] turns it into its own message.)
fn plain_verify_error(e: JobError) -> JobError {
    match e {
        JobError::Verify(_) => JobError::Message(DIDNT_CHECK_OUT.to_owned()),
        other => other,
    }
}

/// One complete encode attempt of `input` into `output`.
struct Encode<'a> {
    input: &'a Path,
    info: &'a MediaInfo,
    plan: &'a CompressPlan,
    /// Bitrate scale for this attempt; applied to the video bitrate, or to the audio bitrate
    /// when there is no video.
    scale: f64,
    /// `-passlogfile` prefix.
    passlog: &'a Path,
    output: &'a Path,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Pass {
    /// Two-pass analysis: video only, output discarded.
    First,
    /// The pass that writes the file (pass 2, or the only pass for audio-only input).
    Final,
}

impl Encode<'_> {
    fn run(
        &self,
        paths: &FfmpegPaths,
        cancel: &CancelToken,
        on_progress: &mut dyn FnMut(JobProgress),
    ) -> Result<(), JobError> {
        let passes: &[Pass] = if self.info.video.is_some() {
            &[Pass::First, Pass::Final]
        } else {
            &[Pass::Final]
        };
        for (i, &pass) in passes.iter().enumerate() {
            let progress = JobProgress {
                pass: i as u8 + 1,
                passes: passes.len() as u8,
                fraction: 0.0,
            };
            run_ffmpeg(
                paths,
                &self.args(pass),
                self.info.duration_s,
                cancel,
                &mut |fraction| {
                    on_progress(JobProgress {
                        fraction,
                        ..progress
                    })
                },
            )?;
        }
        Ok(())
    }

    fn video_bps(&self) -> u64 {
        (self.plan.video_bps as f64 * self.scale) as u64
    }

    fn audio_bps(&self) -> u32 {
        if self.info.video.is_some() {
            self.plan.audio_bps
        } else {
            (self.plan.audio_bps as f64 * self.scale) as u32
        }
    }

    /// `-ac` value: mono when the plan asks for it, stereo when tracks are mixed or a track has
    /// more than two channels, otherwise none (keep the source's mono or stereo).
    fn channels(&self) -> Option<&'static str> {
        let mixed = self.info.audio.len() > 1;
        let surround = self.info.audio.iter().any(|a| a.channels > 2);
        if self.plan.audio_mono {
            Some("1")
        } else if mixed || surround {
            Some("2")
        } else {
            None
        }
    }

    /// ffmpeg arguments for `pass` (after the options `run_ffmpeg` adds itself).
    fn args(&self, pass: Pass) -> Vec<OsString> {
        let mut a = Args::default();
        a.add(["-i"]).path(self.input);

        if let Some(video) = &self.info.video {
            let plan = self.plan;
            a.add(["-map", &format!("0:{}", video.index)]);
            a.add([
                "-vf",
                &format!("scale={}:{},fps={}", plan.width, plan.height, plan.fps),
            ]);
            a.add([
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-b:v",
                &self.video_bps().to_string(),
            ]);
            a.add(["-pix_fmt", "yuv420p"]);
            a.add([
                "-pass",
                if pass == Pass::First { "1" } else { "2" },
                "-passlogfile",
            ])
            .path(self.passlog);
        }

        if pass == Pass::First {
            a.add(["-an", "-f", "null", NULL_OUTPUT]);
            return a.0;
        }

        match self.info.audio.as_slice() {
            [] => {
                a.add(["-an"]);
            }
            [one] => {
                a.add(["-map", &format!("0:{}", one.index)]);
            }
            many => {
                let labels: String = many.iter().map(|s| format!("[0:{}]", s.index)).collect();
                let mix = format!("{labels}amix=inputs={}:normalize=0[mix]", many.len());
                a.add(["-filter_complex", &mix, "-map", "[mix]"]);
            }
        }
        if !self.info.audio.is_empty() {
            a.add(["-c:a", "aac", "-b:a", &self.audio_bps().to_string()]);
            if let Some(channels) = self.channels() {
                a.add(["-ac", channels]);
            }
        }

        a.add(["-movflags", "+faststart", "-f", "mp4"])
            .path(self.output);
        a.0
    }
}

/// Small builder for an ffmpeg argument list.
#[derive(Default)]
struct Args(Vec<OsString>);

impl Args {
    fn add<'s>(&mut self, items: impl IntoIterator<Item = &'s str>) -> &mut Self {
        self.0.extend(items.into_iter().map(OsString::from));
        self
    }

    fn path(&mut self, path: &Path) -> &mut Self {
        self.0.push(path.as_os_str().to_owned());
        self
    }
}

/// Create a new, uniquely named folder under `root` for the two-pass logs.
fn create_work_dir(root: &Path) -> Result<PathBuf, JobError> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for _ in 0..100 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("hasami-2pass-{}-{nanos}-{n}", std::process::id()));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => break,
        }
    }
    Err(JobError::Message(
        "Couldn't create a temporary folder for the encode.".to_owned(),
    ))
}

/// Deletes a file or folder when dropped, so every exit path (error, cancel, panic) cleans up.
/// Deleting something that is already gone (e.g. a temp file that was committed) is a no-op.
struct RemoveOnDrop {
    path: PathBuf,
    is_dir: bool,
    kept: bool,
}

impl RemoveOnDrop {
    fn file(path: PathBuf) -> Self {
        Self {
            path,
            is_dir: false,
            kept: false,
        }
    }

    fn dir(path: PathBuf) -> Self {
        Self {
            path,
            is_dir: true,
            kept: false,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Don't delete it after all.
    fn keep(&mut self) {
        self.kept = true;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        let _ = if self.is_dir {
            fs::remove_dir_all(&self.path)
        } else {
            fs::remove_file(&self.path)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::hidden_command;
    use crate::media::{AudioStream, VideoStream};

    fn too_large() -> JobError {
        JobError::Verify(VerifyError::TooLarge {
            size_bytes: 21_000_000,
            max_bytes: SIZE_LIMIT_BYTES,
        })
    }

    #[test]
    fn retries_three_times_at_lower_bitrate_then_fails() {
        let mut scales = Vec::new();
        let result: Result<(), JobError> = encode_until_fits(|scale| {
            scales.push(scale);
            Err(too_large())
        });
        let err = result.unwrap_err();
        assert_eq!(err.to_string(), NEVER_FIT);
        assert_eq!(scales.len(), 4, "first attempt plus 3 retries");
        for (got, want) in scales.iter().zip([1.0, 0.9, 0.81, 0.729]) {
            assert!((got - want).abs() < 1e-9, "{scales:?}");
        }
    }

    #[test]
    fn retry_stops_at_first_fit() {
        let mut calls = 0;
        let result = encode_until_fits(|scale| {
            calls += 1;
            if calls < 3 {
                Err(too_large())
            } else {
                Ok(scale)
            }
        });
        assert!((result.unwrap() - 0.81).abs() < 1e-9);
        assert_eq!(calls, 3);
    }

    #[test]
    fn other_errors_are_not_retried() {
        let mut calls = 0;
        let result: Result<(), JobError> = encode_until_fits(|_| {
            calls += 1;
            Err(JobError::Cancelled)
        });
        assert!(matches!(result, Err(JobError::Cancelled)));
        assert_eq!(calls, 1);
    }

    #[test]
    fn mp4_keeps_its_exact_name() {
        assert_eq!(
            desired_output(Path::new(r"C:\d\Clip.MP4")),
            Path::new(r"C:\d\Clip.MP4")
        );
        assert_eq!(
            desired_output(Path::new(r"C:\d\Clip.mkv")),
            Path::new(r"C:\d\Clip.mp4")
        );
    }

    fn info(video: bool, audio_tracks: usize) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from("in.mkv"),
            container: "mkv".to_owned(),
            duration_s: 10.0,
            size_bytes: 50_000_000,
            bit_rate: None,
            video: video.then(|| VideoStream {
                index: 0,
                codec: "hevc".to_owned(),
                width: 1920,
                height: 1080,
                fps: 60.0,
                bit_rate: None,
            }),
            audio: (0..audio_tracks)
                .map(|i| AudioStream {
                    index: i + 1,
                    codec: "aac".to_owned(),
                    channels: 2,
                    sample_rate: 48_000,
                    bit_rate: None,
                })
                .collect(),
        }
    }

    fn plan(mono: bool) -> CompressPlan {
        CompressPlan {
            width: 1280,
            height: 720,
            fps: 30.0,
            video_bps: 1_000_000,
            audio_bps: 128_000,
            audio_mono: mono,
            warn_low_res: false,
        }
    }

    fn args(info: &MediaInfo, plan: &CompressPlan, scale: f64, pass: Pass) -> String {
        let job = Encode {
            input: Path::new("in.mkv"),
            info,
            plan,
            scale,
            passlog: Path::new("logs/x264"),
            output: Path::new(".out.mp4.hasami-tmp"),
        };
        job.args(pass)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn first_pass_is_video_only_to_nul() {
        let a = args(&info(true, 3), &plan(false), 1.0, Pass::First);
        assert_eq!(
            a,
            "-i in.mkv -map 0:0 -vf scale=1280:720,fps=30 -c:v libx264 -preset medium -b:v 1000000 \
             -pix_fmt yuv420p -pass 1 -passlogfile logs/x264 -an -f null NUL"
        );
    }

    #[test]
    fn final_pass_mixes_every_track() {
        let a = args(&info(true, 3), &plan(false), 0.9, Pass::Final);
        assert!(a.contains("-b:v 900000 "), "{a}");
        assert!(a.contains("-pass 2 -passlogfile logs/x264"), "{a}");
        assert!(
            a.contains("-filter_complex [0:1][0:2][0:3]amix=inputs=3:normalize=0[mix] -map [mix]"),
            "{a}"
        );
        assert!(
            a.ends_with(
                "-c:a aac -b:a 128000 -ac 2 -movflags +faststart -f mp4 .out.mp4.hasami-tmp"
            ),
            "{a}"
        );
    }

    #[test]
    fn final_pass_maps_a_single_track_directly() {
        let a = args(&info(true, 1), &plan(true), 1.0, Pass::Final);
        assert!(a.contains("-map 0:0 ") && a.contains("-map 0:1 "), "{a}");
        assert!(!a.contains("amix"), "{a}");
        assert!(a.contains("-ac 1 "), "{a}");
    }

    #[test]
    fn final_pass_without_audio_drops_it() {
        let a = args(&info(true, 0), &plan(false), 1.0, Pass::Final);
        assert!(a.contains(" -an "), "{a}");
        assert!(!a.contains("-c:a"), "{a}");
    }

    #[test]
    fn audio_only_is_one_aac_pass_scaled_on_retry() {
        let a = args(&info(false, 1), &plan(false), 0.9, Pass::Final);
        assert_eq!(
            a,
            "-i in.mkv -map 0:1 -c:a aac -b:a 115200 -movflags +faststart -f mp4 .out.mp4.hasami-tmp"
        );
    }

    #[test]
    fn single_track_is_only_downmixed_from_surround() {
        let mut src = info(true, 1);
        for (channels, want) in [(1, None), (2, None), (6, Some("2"))] {
            src.audio[0].channels = channels;
            let a = args(&src, &plan(false), 1.0, Pass::Final);
            let got = a.split(" -ac ").nth(1).map(|rest| &rest[..1]);
            assert_eq!(got, want, "{channels} channels: {a}");
        }
    }

    #[test]
    fn verify_failures_other_than_size_read_as_plain_language() {
        let wrong = JobError::Verify(VerifyError::WrongDuration {
            expected_s: 13.0,
            actual_s: 12.34,
        });
        assert_eq!(plain_verify_error(wrong).to_string(), DIDNT_CHECK_OUT);
        assert!(matches!(
            plain_verify_error(JobError::Cancelled),
            JobError::Cancelled
        ));
    }

    #[test]
    fn remove_on_drop_deletes_unless_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (gone, kept) = (dir.path().join("gone"), dir.path().join("kept"));
        fs::write(&gone, b"x").unwrap();
        fs::write(&kept, b"x").unwrap();
        drop(RemoveOnDrop::file(gone.clone()));
        let mut guard = RemoveOnDrop::file(kept.clone());
        guard.keep();
        drop(guard);
        assert!(!gone.exists());
        assert!(kept.exists());
    }

    /// Write a small `.mkv` (so it is never "already small") into `dir`.
    fn small_mkv(paths: &FfmpegPaths, dir: &Path, name: &str) -> PathBuf {
        let out = dir.join(name);
        let status = hidden_command(&paths.ffmpeg)
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("testsrc2=size=320x180:rate=30:duration=2")
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success());
        out
    }

    #[test]
    fn passlog_folder_is_removed_after_success_and_cancel() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let empty = |root: &Path| fs::read_dir(root).unwrap().next().is_none();

        let input = small_mkv(&paths, dir.path(), "ok.mkv");
        let mut saw_log = false;
        let mut watch = |p: JobProgress| {
            if p.pass == 2 {
                // Pass 1 has written its log into the work folder by now.
                saw_log |= !empty(root.path());
            }
        };
        let outcome = compress_in(
            &paths,
            &input,
            false,
            &CancelToken::new(),
            &mut watch,
            root.path(),
        );
        assert!(
            matches!(outcome, Ok(JobOutcome::Done { .. })),
            "{outcome:?}"
        );
        assert!(saw_log, "the passlog should live under the work root");
        assert!(
            empty(root.path()),
            "passlog folder left behind after success"
        );

        let input = small_mkv(&paths, dir.path(), "cancel.mkv");
        let cancel = CancelToken::new();
        let outcome = compress_in(
            &paths,
            &input,
            false,
            &cancel,
            &mut |_| cancel.cancel(),
            root.path(),
        );
        assert!(matches!(outcome, Err(JobError::Cancelled)), "{outcome:?}");
        assert!(
            empty(root.path()),
            "passlog folder left behind after cancel"
        );
        assert!(input.exists());
    }

    /// 4 s of noisy 1080p30 H.264 at 60 Mbit/s plus AAC (over 20 MB), like `fixture_big_h264`.
    fn big_h264(paths: &FfmpegPaths, dir: &Path) -> PathBuf {
        let out = dir.join("big_h264.mp4");
        let status = hidden_command(&paths.ffmpeg)
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
            .args(["-vf", "noise=alls=60:allf=t"])
            .args(["-c:v", "libx264", "-preset", "ultrafast", "-b:v", "60M"])
            .args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-ac", "2", "-t", "4"])
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success());
        out
    }

    #[test]
    fn encode_to_limit_writes_to_the_given_output() {
        let paths = FfmpegPaths::resolve().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let input = big_h264(&paths, dir.path());
        let before = fs::read(&input).unwrap();
        let info = probe(&paths, &input).unwrap();
        let CompressDecision::Encode(plan) = plan_compress(&info).unwrap() else {
            panic!("a 4 s 60 Mbit/s clip must need encoding");
        };
        let output = dir.path().join("out.mp4");

        let verified = encode_to_limit(
            &paths,
            &input,
            &info,
            &plan,
            &output,
            root.path(),
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();

        let size = fs::metadata(&output).unwrap().len();
        assert!(size <= SIZE_LIMIT_BYTES, "{size} bytes");
        assert_eq!(verified.size_bytes, size);
        assert_eq!(
            fs::read(&input).unwrap(),
            before,
            "the input must be untouched"
        );
        assert!(
            fs::read_dir(root.path()).unwrap().next().is_none(),
            "pass-log folder left behind under the work root"
        );
    }
}
