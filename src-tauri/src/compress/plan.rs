//! Pure ladder/bitrate planner (Task 5).

use crate::media::MediaInfo;

/// Compress output must be at most this many bytes.
pub const SIZE_LIMIT_BYTES: u64 = 20_000_000;

/// Fraction of the limit we actually budget for, leaving room for container overhead.
const BUDGET_FACTOR: f64 = 0.96;
/// Below this video bitrate the result is not watchable.
const MIN_VIDEO_BPS: f64 = 50_000.0;
/// Minimum bits per pixel per frame for a rung to be considered good enough.
const MIN_BITS_PER_PIXEL: f64 = 0.05;
/// Audio may use at most this share of the budget.
const AUDIO_BUDGET_SHARE: f64 = 0.10;

/// `(bits per second, mono)` audio options, best first.
const AUDIO_OPTIONS: [(u32, bool); 4] =
    [(128_000, false), (96_000, false), (64_000, false), (32_000, true)];
const AUDIO_FALLBACK: (u32, bool) = (32_000, true);

/// `(height, fps)` ladder, best first.
const LADDER: [(u32, f64); 7] =
    [(1080, 60.0), (1080, 30.0), (720, 60.0), (720, 30.0), (480, 30.0), (360, 30.0), (240, 15.0)];

/// What to encode. Dimensions are even. `width`, `height` and `fps` are 0 when the input has
/// no video stream.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressPlan {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_bps: u64,
    pub audio_bps: u32,
    pub audio_mono: bool,
    /// The result will be below 480p.
    pub warn_low_res: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompressDecision {
    /// The input already meets the limit in a compatible format; nothing to do.
    AlreadySmall,
    Encode(CompressPlan),
}

#[derive(Debug, thiserror::Error)]
pub enum CompressError {
    #[error("This video is too long to fit in 20 MB at any watchable quality.")]
    TooLong,

    #[error("The video's length could not be determined, so it can't be compressed.")]
    UnknownDuration,
}

/// Decide whether and how to re-encode `info` so the output is at most [`SIZE_LIMIT_BYTES`].
pub fn plan_compress(info: &MediaInfo) -> Result<CompressDecision, CompressError> {
    let within_limit = info.size_bytes <= SIZE_LIMIT_BYTES;

    if within_limit
        && info.container == "mp4"
        && info.video.as_ref().map_or(true, |v| v.codec == "h264")
        && info.audio.len() <= 1
        && info.audio.iter().all(|a| a.codec == "aac")
    {
        return Ok(CompressDecision::AlreadySmall);
    }

    let duration = info.duration_s;
    if !duration.is_finite() || duration <= 0.0 {
        return Err(CompressError::UnknownDuration);
    }

    let budget_bits = SIZE_LIMIT_BYTES as f64 * 8.0 * BUDGET_FACTOR;

    let (audio_bps, audio_mono) = if info.audio.is_empty() {
        (0, false)
    } else {
        AUDIO_OPTIONS
            .iter()
            .copied()
            .find(|&(bps, _)| bps as f64 * duration <= budget_bits * AUDIO_BUDGET_SHARE)
            .unwrap_or(AUDIO_FALLBACK)
    };

    let video_bps = (budget_bits - audio_bps as f64 * duration) / duration;
    if video_bps < MIN_VIDEO_BPS {
        return Err(CompressError::TooLong);
    }
    let mut video_bps = video_bps as u64;

    let Some(video) = &info.video else {
        return Ok(CompressDecision::Encode(CompressPlan {
            width: 0,
            height: 0,
            fps: 0.0,
            video_bps,
            audio_bps,
            audio_mono,
            warn_low_res: false,
        }));
    };

    let (width, height, fps) = pick_rung(video.width, video.height, video.fps, video_bps as f64);

    if within_limit {
        if let Some(source_bps) = video.bit_rate {
            video_bps = video_bps.min(source_bps);
        }
    }

    Ok(CompressDecision::Encode(CompressPlan {
        width,
        height,
        fps,
        video_bps,
        audio_bps,
        audio_mono,
        warn_low_res: height < 480,
    }))
}

/// Round to the nearest even number, at least 2.
fn even(x: f64) -> u32 {
    (((x / 2.0).round() as u32) * 2).max(2)
}

/// The first ladder rung with enough bits per pixel, or the last allowed one. Rungs taller
/// than the source are skipped, rung fps is clamped to the source fps, and rungs that become
/// identical after clamping are dropped.
fn pick_rung(src_w: u32, src_h: u32, src_fps: f64, video_bps: f64) -> (u32, u32, f64) {
    let src_fps = if src_fps.is_finite() && src_fps > 0.0 { src_fps } else { 30.0 };

    let mut rungs: Vec<(u32, u32, f64)> = Vec::new();
    for &(h, fps) in &LADDER {
        if h > src_h {
            continue;
        }
        let w = even(src_w as f64 * h as f64 / src_h as f64);
        let rung = (w, h, fps.min(src_fps));
        if !rungs.contains(&rung) {
            rungs.push(rung);
        }
    }
    if rungs.is_empty() {
        // Source is shorter than the smallest rung: keep its size.
        rungs.push((even(src_w as f64), even(src_h as f64), 15.0_f64.min(src_fps)));
    }

    rungs
        .iter()
        .copied()
        .find(|&(w, h, fps)| video_bps / (w as f64 * h as f64 * fps) >= MIN_BITS_PER_PIXEL)
        .unwrap_or_else(|| *rungs.last().expect("rungs is never empty"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::media::{AudioStream, VideoStream};

    fn info(
        container: &str,
        duration_s: f64,
        size_bytes: u64,
        video: Option<(u32, u32, f64, &str, Option<u64>)>,
        audio_tracks: usize,
    ) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from(format!("in.{container}")),
            container: container.to_owned(),
            duration_s,
            size_bytes,
            bit_rate: None,
            video: video.map(|(width, height, fps, codec, bit_rate)| VideoStream {
                index: 0,
                codec: codec.to_owned(),
                width,
                height,
                fps,
                bit_rate,
            }),
            audio: (0..audio_tracks)
                .map(|i| AudioStream {
                    index: i + 1,
                    codec: "aac".to_owned(),
                    channels: 2,
                    sample_rate: 48_000,
                    bit_rate: Some(128_000),
                })
                .collect(),
        }
    }

    fn src_1080p60(duration_s: f64, audio_tracks: usize) -> MediaInfo {
        info(
            "mp4",
            duration_s,
            412_000_000,
            Some((1920, 1080, 60.0, "h264", Some(35_000_000))),
            audio_tracks,
        )
    }

    fn encode(decision: Result<CompressDecision, CompressError>) -> CompressPlan {
        match decision.expect("plan failed") {
            CompressDecision::Encode(plan) => plan,
            other => panic!("expected Encode, got {other:?}"),
        }
    }

    #[test]
    fn plan_90s_1080p60() {
        let plan = encode(plan_compress(&src_1080p60(89.3, 3)));
        assert_eq!((plan.width, plan.height), (1280, 720));
        assert_eq!(plan.fps, 30.0);
        assert_eq!(plan.audio_bps, 128_000);
        assert!(!plan.audio_mono);
        assert!(!plan.warn_low_res);
    }

    #[test]
    fn plan_10min() {
        let plan = encode(plan_compress(&src_1080p60(600.0, 1)));
        assert_eq!((plan.width, plan.height), (426, 240));
        assert_eq!(plan.fps, 15.0);
        assert_eq!(plan.audio_bps, 32_000);
        assert!(plan.audio_mono);
        assert!(plan.warn_low_res);
    }

    #[test]
    fn plan_3h_too_long() {
        let err = plan_compress(&src_1080p60(10_800.0, 1)).unwrap_err();
        assert!(matches!(err, CompressError::TooLong));
        let msg = err.to_string();
        assert!(msg.contains("too long to fit in 20 MB"), "{msg}");
        assert!(!msg.contains("original"), "the window adds that line itself: {msg}");
    }

    #[test]
    fn plan_never_upscales() {
        let src = info("mp4", 30.0, 100_000_000, Some((1280, 720, 30.0, "h264", None)), 1);
        let plan = encode(plan_compress(&src));
        assert!(plan.height <= 720 && plan.width <= 1280);
        assert!(plan.fps <= 30.0);
    }

    #[test]
    fn plan_25fps_source_uses_clamped_rungs() {
        let src = info("mp4", 30.0, 100_000_000, Some((1920, 1080, 25.0, "h264", None)), 1);
        let plan = encode(plan_compress(&src));
        assert!(plan.height > 240, "fell to {}p", plan.height);
        assert!(plan.fps <= 25.0 && plan.fps > 15.0, "fps was {}", plan.fps);
    }

    #[test]
    fn plan_already_small() {
        let small = |container: &str| {
            info(container, 10.0, 2_000_000, Some((1280, 720, 30.0, "h264", Some(1_200_000))), 1)
        };
        assert_eq!(plan_compress(&small("mp4")).unwrap(), CompressDecision::AlreadySmall);

        let plan = encode(plan_compress(&small("mkv")));
        assert!(plan.video_bps <= 1_200_000, "video_bps was {}", plan.video_bps);
    }

    #[test]
    fn plan_small_but_not_compatible_is_encoded() {
        let mut hevc = info("mp4", 10.0, 2_000_000, Some((1280, 720, 30.0, "hevc", None)), 1);
        assert!(matches!(plan_compress(&hevc).unwrap(), CompressDecision::Encode(_)));
        hevc.video.as_mut().unwrap().codec = "h264".to_owned();
        hevc.audio.push(hevc.audio[0].clone());
        assert!(matches!(plan_compress(&hevc).unwrap(), CompressDecision::Encode(_)));
        hevc.audio.pop();
        hevc.audio[0].codec = "opus".to_owned();
        assert!(matches!(plan_compress(&hevc).unwrap(), CompressDecision::Encode(_)));
    }

    #[test]
    fn plan_over_limit_is_never_already_small() {
        let src = info("mp4", 10.0, 20_000_001, Some((1280, 720, 30.0, "h264", None)), 1);
        assert!(matches!(plan_compress(&src).unwrap(), CompressDecision::Encode(_)));
    }

    #[test]
    fn plan_no_audio() {
        let src = info("mp4", 60.0, 90_000_000, Some((1920, 1080, 30.0, "h264", None)), 0);
        let plan = encode(plan_compress(&src));
        assert_eq!(plan.audio_bps, 0);
        assert!(!plan.audio_mono);
    }

    #[test]
    fn plan_dimensions_are_even_and_keep_aspect() {
        // 4:3 portrait-ish odd-ratio source.
        let src = info("mp4", 600.0, 100_000_000, Some((1000, 750, 30.0, "h264", None)), 1);
        let plan = encode(plan_compress(&src));
        assert_eq!((plan.width % 2, plan.height % 2), (0, 0));
        let ratio = plan.width as f64 / plan.height as f64;
        assert!((ratio - 4.0 / 3.0).abs() < 0.02, "ratio was {ratio}");
    }

    #[test]
    fn plan_output_fits_budget() {
        for &dur in &[30.0, 89.3, 300.0, 600.0, 1200.0] {
            let plan = encode(plan_compress(&src_1080p60(dur, 2)));
            let bits = (plan.video_bps + plan.audio_bps as u64) as f64 * dur;
            assert!(bits / 8.0 <= SIZE_LIMIT_BYTES as f64 * 0.96 + 1.0, "{dur}s: {bits}");
        }
    }
}
