//! The export request the editor sends to Rust, the ffmpeg `-filter_complex` graph built from it,
//! and the Discord-safe H.264 encoder choice (AMD AMF hardware when available, else software).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::media::Crop;
use crate::media::{hidden_command, FfmpegPaths, MediaInfo};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportMode {
    Quick,
    SaveRaw,
    Audio,
}

/// A kept stretch of the source video, in source seconds. Pieces play back to back.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct VideoPiece {
    pub src_start: f64,
    pub src_end: f64,
}

/// A kept stretch of one audio track: source seconds `src_start..src_end`, placed at `at`
/// seconds on the output timeline.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AudioPiece {
    pub at: f64,
    pub src_start: f64,
    pub src_end: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AudioTrackRequest {
    /// Index among the source's audio streams (`0:a:<index>`).
    pub index: usize,
    pub gain_db: f64,
    pub pieces: Vec<AudioPiece>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ExportRequest {
    pub session: u64,
    pub input: PathBuf,
    pub mode: ExportMode,
    pub fit_20mb: bool,
    /// Length of the edited output in seconds.
    pub duration_s: f64,
    pub video: Vec<VideoPiece>,
    pub audio: Vec<AudioTrackRequest>,
    /// Gain in dB applied to the final mix of every track. Never −∞: at that setting the editor
    /// sends no audio tracks instead.
    #[serde(default)]
    pub master_db: f64,
    /// The picture area to keep, in source pixels; it is scaled back up to the source's size.
    /// `None` keeps the whole frame.
    #[serde(default)]
    pub crop: Option<Crop>,
}

/// A `-filter_complex` string and the labels to `-map`; `None` when there is no such stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    pub filter: String,
    pub video_out: Option<String>,
    pub audio_out: Option<String>,
}

/// Format `n` with 6 decimals, trailing zeros (and a bare dot) trimmed: `2.0` → `2`, `0.5` → `0.5`.
fn num(n: f64) -> String {
    let s = format!("{n:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    // "-0.000000" trims to "-0"; ffmpeg reads that fine, but keep the graph tidy.
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

/// Build the filter graph for `req`: video pieces trimmed and concatenated into `[vout]`, then,
/// with a crop, cut to it (see [`crop_filter`]; `source_size` is the source's width and height),
/// or without one, cut to even if the source is an odd size; each
/// audio track's pieces trimmed, delayed to their place on the timeline, mixed and gained into
/// `[t<k>]`; all tracks mixed, given the master gain, padded with silence and cut to exactly the
/// output length (kept audio may end before the video does), and made stereo into `[aout]`.
pub fn build_graph(req: &ExportRequest, source_size: (u32, u32)) -> Graph {
    let mut parts: Vec<String> = Vec::new();

    let video_out = if req.video.is_empty() {
        None
    } else {
        for (i, p) in req.video.iter().enumerate() {
            parts.push(format!(
                "[0:v:0]trim=start={}:end={},setpts=PTS-STARTPTS[v{i}]",
                num(p.src_start),
                num(p.src_end)
            ));
        }
        let labels: String = (0..req.video.len()).map(|i| format!("[v{i}]")).collect();
        let mut chain = Vec::new();
        if req.video.len() > 1 {
            chain.push(format!("concat=n={}:v=1:a=0", req.video.len()));
        }
        if let Some(crop) = req.crop {
            chain.push(crop_filter(crop, source_size));
        } else if source_size != output_size(None, source_size) {
            // 4:2:0 H.264 can't be an odd size: drop the last row or column.
            let (w, h) = output_size(None, source_size);
            chain.push(format!("crop={w}:{h}:0:0"));
        }
        if chain.is_empty() {
            chain.push("null".to_owned());
        }
        parts.push(format!("{labels}{}[vout]", chain.join(",")));
        Some("[vout]".to_owned())
    };

    let mut track_labels: Vec<String> = Vec::new();
    for (k, track) in req.audio.iter().enumerate() {
        if track.pieces.is_empty() {
            continue;
        }
        for (j, p) in track.pieces.iter().enumerate() {
            parts.push(format!(
                "[0:a:{}]atrim=start={}:end={},asetpts=PTS-STARTPTS,adelay={}:all=1[a{k}_{j}]",
                track.index,
                num(p.src_start),
                num(p.src_end),
                (p.at * 1000.0).round() as i64
            ));
        }
        let inputs: String = (0..track.pieces.len()).map(|j| format!("[a{k}_{j}]")).collect();
        let mix = if track.pieces.len() > 1 {
            format!("amix=inputs={}:normalize=0:duration=longest,", track.pieces.len())
        } else {
            String::new()
        };
        parts.push(format!("{inputs}{mix}volume={:.2}dB[t{k}]", track.gain_db));
        track_labels.push(format!("[t{k}]"));
    }

    let audio_out = if track_labels.is_empty() {
        None
    } else {
        let mix = if track_labels.len() > 1 {
            format!("amix=inputs={}:normalize=0:duration=longest,", track_labels.len())
        } else {
            String::new()
        };
        let master = if req.master_db != 0.0 { format!("volume={:.2}dB,", req.master_db) } else { String::new() };
        let dur = num(req.duration_s);
        parts.push(format!(
            "{}{mix}{master}apad=whole_dur={dur},atrim=end={dur},aformat=channel_layouts=stereo[aout]",
            track_labels.concat()
        ));
        Some("[aout]".to_owned())
    };

    Graph { filter: parts.join(";"), video_out, audio_out }
}

/// How far (as a fraction) a crop's aspect ratio may be from the source's and still be scaled
/// back up to the source's size.
const SAME_ASPECT_TOLERANCE: f64 = 0.01;

/// `n` rounded down to even, as 4:2:0 H.264 needs.
fn even(n: u32) -> u32 {
    n & !1
}

/// The filters that cut the picture to `crop`. When the crop has the source's aspect ratio
/// (within 1%), the picture is scaled back up to `source_size` so it fills the frame, the way an
/// OBS canvas bigger than the game leaves it; otherwise it keeps the crop's own size, so a
/// pillarboxed or letterboxed picture is never stretched. Sizes are kept even.
pub fn crop_filter(crop: Crop, source_size: (u32, u32)) -> String {
    let Crop { x, y, w, h } = crop;
    let (sw, sh) = source_size;
    let (w, h) = (even(w), even(h));
    if same_aspect((w, h), source_size) {
        format!("crop={w}:{h}:{x}:{y},scale={}:{}:flags=lanczos,setsar=1", even(sw), even(sh))
    } else {
        format!("crop={w}:{h}:{x}:{y}")
    }
}

/// Whether a `size` picture has the `source_size` frame's aspect ratio, within 1%.
fn same_aspect((w, h): (u32, u32), (sw, sh): (u32, u32)) -> bool {
    h > 0
        && sh > 0
        && ((f64::from(w) / f64::from(h)) / (f64::from(sw) / f64::from(sh)) - 1.0).abs() <= SAME_ASPECT_TOLERANCE
}

/// The width and height a video export comes out at: the source's, or with a crop of another
/// aspect ratio the crop's own (see [`crop_filter`]); always rounded down to even.
pub fn output_size(crop: Option<Crop>, source_size: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = source_size;
    match crop {
        Some(Crop { w, h, .. }) if !same_aspect((even(w), even(h)), source_size) => (even(w), even(h)),
        _ => (even(sw), even(sh)),
    }
}

/// A video encoder name for `-c:v` and the rate/format arguments that go with it.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoEncoder {
    pub codec: &'static str,
    pub args: Vec<String>,
}

/// How long the one-frame test encode in [`encoder_available`] may take. A stuck hardware driver
/// would otherwise hold up the export, and every job queued behind it, for good.
const ENCODER_TEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Whether the bundled ffmpeg can encode with `name` on this machine: a one-frame test encode
/// (given up after [`ENCODER_TEST_TIMEOUT`]), remembered per encoder name for the life of the
/// process.
pub fn encoder_available(paths: &FfmpegPaths, name: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let lock = || cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(&known) = lock().get(name) {
        return known;
    }
    // Not holding the lock while ffmpeg runs; two threads racing just both run the test.
    let mut test = hidden_command(&paths.ffmpeg);
    test.args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=640x360:r=30:d=1"])
        .args(["-frames:v", "1", "-pix_fmt", "yuv420p", "-c:v", name, "-f", "null", "-"]);
    let ok = succeeds_within(test, ENCODER_TEST_TIMEOUT);
    lock().insert(name.to_owned(), ok);
    ok
}

/// Run `command` (its output discarded) and say whether it exits successfully within `limit`. One
/// still running then is killed, and counts as a failure.
fn succeeds_within(mut command: Command, limit: Duration) -> bool {
    let spawned = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    let Ok(mut child) = spawned else {
        return false;
    };
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// The largest picture AMD AMF encodes as H.264 (wider than 4096 or taller than 2304 fails, e.g.
/// a 5120×1440 super-ultrawide canvas); bigger exports go to libx264.
pub const AMF_H264_MAX: (u32, u32) = (4096, 2304);

/// The H.264 encoder for a video export of a `size` (width, height) picture at `bitrate` bits/s
/// (average; peak 1.5×): AMD AMF when `hw("h264_amf")` says it works here (normally
/// [`encoder_available`]) and the picture is no bigger than [`AMF_H264_MAX`], else
/// [`software_h264`]. Always 8-bit 4:2:0, which every Discord client plays.
pub fn h264_encoder(bitrate: u64, size: (u32, u32), hw: &dyn Fn(&str) -> bool) -> VideoEncoder {
    let (w, h) = size;
    let fits_amf = w <= AMF_H264_MAX.0 && h <= AMF_H264_MAX.1;
    if fits_amf && hw("h264_amf") {
        let b = bitrate;
        let rate = args(&["-b:v", &b.to_string(), "-maxrate", &(b + b / 2).to_string()]);
        VideoEncoder {
            codec: "h264_amf",
            args: [args(&["-rc", "vbr_peak"]), rate, args(&["-pix_fmt", "yuv420p"])].concat(),
        }
    } else {
        software_h264(bitrate)
    }
}

/// libx264 at `bitrate` bits/s (average; peak 1.5×), 8-bit 4:2:0: the encoder that works
/// everywhere, and what a failed hardware render is retried with.
#[rustfmt::skip] // one option and its value per pair
pub fn software_h264(bitrate: u64) -> VideoEncoder {
    let b = bitrate;
    VideoEncoder {
        codec: "libx264",
        args: args(&[
            "-preset", "medium",
            "-b:v", &b.to_string(),
            "-maxrate", &(b + b / 2).to_string(),
            "-bufsize", &(b * 2).to_string(),
            "-pix_fmt", "yuv420p",
        ]),
    }
}

/// Source codecs that pack more picture into each bit than H.264 does; re-encoding them as H.264
/// at the same bitrate would lose quality.
const DENSER_CODECS: [&str; 3] = ["hevc", "av1", "vp9"];
/// How much more bitrate H.264 gets than a [`DENSER_CODECS`] source had. The editor's size
/// estimate (`exportBitrateFactor` in `src/editor/store.svelte.ts`) mirrors this and the list.
const DENSER_CODEC_FACTOR: f64 = 1.3;

/// The bitrate a video export encodes at: [`source_video_bitrate`], ×1.3 for HEVC, AV1 and VP9.
pub fn export_bitrate(info: &MediaInfo) -> u64 {
    let bps = source_video_bitrate(info);
    let codec = info.video.as_ref().map_or("", |v| v.codec.as_str());
    if DENSER_CODECS.contains(&codec) {
        (bps as f64 * DENSER_CODEC_FACTOR).round() as u64
    } else {
        bps
    }
}

/// Audio bitrate assumed per source track when estimating the video bitrate.
const AUDIO_TRACK_BPS: u64 = 192_000;

/// The source's video bitrate in bits/s: the video stream's own figure, else the container's
/// minus 192 kbit/s per audio track, else 8 Mbit/s; never below 1 Mbit/s.
pub fn source_video_bitrate(info: &MediaInfo) -> u64 {
    let bps = info
        .video
        .as_ref()
        .and_then(|v| v.bit_rate)
        .or_else(|| info.bit_rate.map(|b| b.saturating_sub(AUDIO_TRACK_BPS * info.audio.len() as u64)))
        .unwrap_or(8_000_000);
    bps.max(1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{AudioStream, VideoStream};

    fn req(video: Vec<VideoPiece>, audio: Vec<AudioTrackRequest>, duration_s: f64) -> ExportRequest {
        ExportRequest {
            session: 1,
            input: PathBuf::from("in.mp4"),
            mode: ExportMode::Quick,
            fit_20mb: false,
            duration_s,
            video,
            audio,
            master_db: 0.0,
            crop: None,
        }
    }

    const FRAME: (u32, u32) = (1920, 1080);

    fn vp(src_start: f64, src_end: f64) -> VideoPiece {
        VideoPiece { src_start, src_end }
    }

    fn ap(at: f64, src_start: f64, src_end: f64) -> AudioPiece {
        AudioPiece { at, src_start, src_end }
    }

    #[test]
    fn graph_single_piece_no_audio() {
        let g = build_graph(&req(vec![vp(2.0, 5.0)], vec![], 3.0), FRAME);
        assert_eq!(g.filter, "[0:v:0]trim=start=2:end=5,setpts=PTS-STARTPTS[v0];[v0]null[vout]");
        assert_eq!(g.video_out.as_deref(), Some("[vout]"));
        assert_eq!(g.audio_out, None);
    }

    #[test]
    fn graph_concat_and_mix() {
        let audio = vec![
            AudioTrackRequest { index: 0, gain_db: 0.0, pieces: vec![ap(0.0, 0.0, 12.0), ap(12.0, 31.0, 90.0)] },
            AudioTrackRequest { index: 2, gain_db: -6.0, pieces: vec![ap(0.0, 0.0, 12.0)] },
        ];
        let g = build_graph(&req(vec![vp(0.0, 12.0), vp(31.0, 90.0)], audio, 71.0), FRAME);
        let f = &g.filter;
        assert!(f.contains("concat=n=2:v=1:a=0[vout]"), "{f}");
        assert!(f.contains("[0:a:0]atrim=start=31:end=90,asetpts=PTS-STARTPTS,adelay=12000:all=1"), "{f}");
        assert!(f.contains("[0:a:2]atrim=start=0:end=12"), "{f}");
        assert!(f.contains("volume=-6.00dB"), "{f}");
        assert!(f.contains("amix=inputs=2:normalize=0:duration=longest"), "{f}");
        assert!(f.ends_with("apad=whole_dur=71,atrim=end=71,aformat=channel_layouts=stereo[aout]"), "{f}");
        assert_eq!(g.audio_out.as_deref(), Some("[aout]"));
    }

    #[test]
    fn graph_master_gain_goes_on_the_final_mix_before_the_pad() {
        let track = |index| AudioTrackRequest { index, gain_db: 0.0, pieces: vec![ap(0.0, 0.0, 3.0)] };
        let louder = ExportRequest { master_db: 6.0, ..req(vec![vp(0.0, 3.0)], vec![track(0), track(1)], 3.0) };
        let f = build_graph(&louder, FRAME).filter;
        assert!(
            f.ends_with(
                "[t0][t1]amix=inputs=2:normalize=0:duration=longest,volume=6.00dB,\
                 apad=whole_dur=3,atrim=end=3,aformat=channel_layouts=stereo[aout]"
            ),
            "{f}"
        );

        let single = ExportRequest { master_db: -2.5, ..req(vec![vp(0.0, 3.0)], vec![track(0)], 3.0) };
        let f = build_graph(&single, FRAME).filter;
        assert!(
            f.ends_with("[t0]volume=-2.50dB,apad=whole_dur=3,atrim=end=3,aformat=channel_layouts=stereo[aout]"),
            "{f}"
        );

        let unity = build_graph(&req(vec![vp(0.0, 3.0)], vec![track(0), track(1)], 3.0), FRAME).filter;
        assert_eq!(unity.matches("volume=").count(), 2, "only the two track gains: {unity}");
        assert!(unity.contains("duration=longest,apad="), "{unity}");
    }

    #[test]
    fn master_db_defaults_to_zero() {
        let json =
            r#"{"session":1,"input":"in.mp4","mode":"quick","fit_20mb":false,"duration_s":1,"video":[],"audio":[]}"#;
        let req: ExportRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.master_db, 0.0);
    }

    #[test]
    fn graph_audio_mode_has_no_video() {
        let audio = vec![AudioTrackRequest { index: 0, gain_db: 0.0, pieces: vec![ap(0.0, 1.0, 4.0)] }];
        let g = build_graph(&req(vec![], audio, 3.0), FRAME);
        assert_eq!(g.video_out, None);
        assert_eq!(g.audio_out.as_deref(), Some("[aout]"));
        assert!(!g.filter.contains("[0:v:0]"), "{}", g.filter);
    }

    #[test]
    fn graph_numbers_are_trimmed_not_rounded_away() {
        let g = build_graph(&req(vec![vp(1.5, 2.123_456_789)], vec![], 0.62), FRAME);
        assert!(g.filter.contains("trim=start=1.5:end=2.123457"), "{}", g.filter);
    }

    #[test]
    fn graph_crop_cuts_the_picture_and_scales_it_back_up() {
        let crop = Some(Crop { x: 0, y: 0, w: 1440, h: 810 });
        let one = ExportRequest { crop, ..req(vec![vp(2.0, 5.0)], vec![], 3.0) };
        assert_eq!(
            build_graph(&one, FRAME).filter,
            "[0:v:0]trim=start=2:end=5,setpts=PTS-STARTPTS[v0];\
             [v0]crop=1440:810:0:0,scale=1920:1080:flags=lanczos,setsar=1[vout]"
        );

        let two = ExportRequest {
            crop: Some(Crop { x: 8, y: 6, w: 640, h: 360 }),
            ..req(vec![vp(0.0, 1.0), vp(2.0, 3.0)], vec![], 2.0)
        };
        let f = build_graph(&two, (1280, 720)).filter;
        assert!(
            f.ends_with("[v0][v1]concat=n=2:v=1:a=0,crop=640:360:8:6,scale=1280:720:flags=lanczos,setsar=1[vout]"),
            "{f}"
        );
    }

    #[test]
    fn crop_of_another_aspect_keeps_its_own_size() {
        // A 4:3 game pillarboxed in 16:9, and a 21:9 letterbox: cut out, never stretched.
        assert_eq!(crop_filter(Crop { x: 240, y: 0, w: 1440, h: 1080 }, FRAME), "crop=1440:1080:240:0");
        assert_eq!(crop_filter(Crop { x: 0, y: 140, w: 1920, h: 800 }, FRAME), "crop=1920:800:0:140");
        // Odd sizes are rounded down to even.
        assert_eq!(crop_filter(Crop { x: 241, y: 1, w: 1441, h: 1079 }, FRAME), "crop=1440:1078:241:1");
    }

    #[test]
    fn crop_within_one_percent_of_the_frame_aspect_is_scaled_back_up() {
        let scaled = |w, h| crop_filter(Crop { x: 0, y: 0, w, h }, FRAME).contains("scale=1920:1080");
        assert!(scaled(1440, 810));
        // 1.778 × 1.01 ≈ 1.796 and × 0.99 ≈ 1.760.
        assert!(scaled(1452, 810), "1.793 is within 1%");
        assert!(scaled(1426, 810), "1.760 is within 1%");
        assert!(!scaled(1456, 810), "1.798 is not");
        assert!(!scaled(1424, 810), "1.758 is not");
        // An odd source size is scaled to even.
        assert_eq!(
            crop_filter(Crop { x: 0, y: 0, w: 960, h: 540 }, (1921, 1081)),
            "crop=960:540:0:0,scale=1920:1080:flags=lanczos,setsar=1"
        );
    }

    #[test]
    fn crop_defaults_to_none_and_parses() {
        let json =
            r#"{"session":1,"input":"in.mp4","mode":"quick","fit_20mb":false,"duration_s":1,"video":[],"audio":[]}"#;
        assert_eq!(serde_json::from_str::<ExportRequest>(json).unwrap().crop, None);
        let json = r#"{"session":1,"input":"in.mp4","mode":"quick","fit_20mb":false,"duration_s":1,"video":[],"audio":[],
                       "crop":{"x":0,"y":0,"w":1440,"h":810}}"#;
        let crop = serde_json::from_str::<ExportRequest>(json).unwrap().crop;
        assert_eq!(crop, Some(Crop { x: 0, y: 0, w: 1440, h: 810 }));
    }

    #[test]
    fn a_test_encode_that_hangs_counts_as_unavailable() {
        let paths = FfmpegPaths::resolve().unwrap();
        // Ten seconds of real-time input stands in for a stuck driver.
        let mut slow = hidden_command(&paths.ffmpeg);
        slow.args(["-hide_banner", "-loglevel", "error", "-re", "-f", "lavfi", "-i", "color=s=64x64:d=10"]);
        slow.args(["-f", "null", "-"]);
        let started = std::time::Instant::now();
        assert!(!succeeds_within(slow, Duration::from_millis(300)));
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());

        let mut quick = hidden_command(&paths.ffmpeg);
        quick.args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=s=64x64:d=0.1", "-f", "null", "-"]);
        assert!(succeeds_within(quick, ENCODER_TEST_TIMEOUT));
    }

    #[test]
    fn h264_on_amf_when_it_works() {
        let e = h264_encoder(10_000_000, FRAME, &|n| n == "h264_amf");
        assert_eq!(e.codec, "h264_amf");
        assert_eq!(e.args, ["-rc", "vbr_peak", "-b:v", "10000000", "-maxrate", "15000000", "-pix_fmt", "yuv420p"]);
    }

    #[test]
    fn h264_bigger_than_amf_can_take_goes_to_libx264() {
        let codec = |size| h264_encoder(10_000_000, size, &|_| true).codec;
        for size in [(1920, 1080), (2560, 1440), (3840, 2160), (4096, 2304), (1080, 1920)] {
            assert_eq!(codec(size), "h264_amf", "{size:?}");
        }
        // Super-ultrawide, 8K, one pixel over in either direction, and portrait 4K.
        for size in [(5120, 1440), (7680, 2160), (4098, 2160), (3840, 2306), (2160, 3840)] {
            assert_eq!(codec(size), "libx264", "{size:?}");
        }
        assert_eq!(h264_encoder(10_000_000, (5120, 1440), &|_| true), software_h264(10_000_000));
    }

    #[test]
    fn output_size_follows_the_crop() {
        assert_eq!(output_size(None, FRAME), FRAME);
        assert_eq!(output_size(None, (5121, 1441)), (5120, 1440), "rounded down to even");
        // Same aspect: scaled back up to the source's size.
        assert_eq!(output_size(Some(Crop { x: 0, y: 0, w: 1440, h: 810 }), FRAME), FRAME);
        assert_eq!(output_size(Some(Crop { x: 0, y: 0, w: 3840, h: 1080 }), (5120, 1440)), (5120, 1440));
        // Another aspect: the crop's own size, so a crop can bring a big canvas within AMF's reach.
        assert_eq!(output_size(Some(Crop { x: 1280, y: 0, w: 2560, h: 1440 }), (5120, 1440)), (2560, 1440));
        assert_eq!(output_size(Some(Crop { x: 241, y: 1, w: 1441, h: 1079 }), FRAME), (1440, 1078));
    }

    #[test]
    fn graph_without_a_crop_makes_an_odd_source_even() {
        let f = build_graph(&req(vec![vp(0.0, 1.0)], vec![], 1.0), (1281, 721)).filter;
        assert!(f.ends_with("[v0]crop=1280:720:0:0[vout]"), "{f}");
        let f = build_graph(&req(vec![vp(0.0, 1.0)], vec![], 1.0), (1280, 721)).filter;
        assert!(f.ends_with("[v0]crop=1280:720:0:0[vout]"), "{f}");
        let f = build_graph(&req(vec![vp(0.0, 1.0)], vec![], 1.0), (1280, 720)).filter;
        assert!(f.ends_with("[v0]null[vout]"), "{f}");
    }

    #[test]
    fn h264_falls_back_to_libx264() {
        let e = h264_encoder(10_000_000, FRAME, &|_| false);
        assert_eq!(e.codec, "libx264");
        assert_eq!(
            e.args,
            [
                "-preset", "medium", "-b:v", "10000000", "-maxrate", "15000000", "-bufsize", "20000000", "-pix_fmt",
                "yuv420p"
            ]
        );
    }

    fn info(video_bps: Option<u64>, total_bps: Option<u64>, tracks: usize) -> MediaInfo {
        info_with("h264", video_bps, total_bps, tracks)
    }

    fn info_with(codec: &str, video_bps: Option<u64>, total_bps: Option<u64>, tracks: usize) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from("in.mp4"),
            container: "mp4".into(),
            duration_s: 10.0,
            size_bytes: 1,
            bit_rate: total_bps,
            video: Some(VideoStream {
                index: 0,
                codec: codec.into(),
                width: 1920,
                height: 1080,
                fps: 30.0,
                bit_rate: video_bps,
            }),
            audio: (0..tracks)
                .map(|i| AudioStream {
                    index: i + 1,
                    codec: "aac".into(),
                    channels: 2,
                    sample_rate: 48000,
                    bit_rate: None,
                })
                .collect(),
        }
    }

    #[test]
    fn bitrate_fallbacks() {
        assert_eq!(source_video_bitrate(&info(Some(9_000_000), Some(10_000_000), 3)), 9_000_000);
        assert_eq!(source_video_bitrate(&info(None, Some(10_000_000), 3)), 10_000_000 - 576_000);
        assert_eq!(source_video_bitrate(&info(None, None, 3)), 8_000_000);
        assert_eq!(source_video_bitrate(&info(Some(200_000), None, 1)), 1_000_000);
    }

    #[test]
    fn denser_codecs_get_more_bitrate() {
        for codec in ["hevc", "av1", "vp9"] {
            assert_eq!(export_bitrate(&info_with(codec, Some(10_000_000), None, 1)), 13_000_000, "{codec}");
        }
        for codec in ["h264", "vp8", "prores"] {
            assert_eq!(export_bitrate(&info_with(codec, Some(10_000_000), None, 1)), 10_000_000, "{codec}");
        }
        // The 1 Mbit/s floor comes first.
        assert_eq!(export_bitrate(&info_with("hevc", Some(200_000), None, 1)), 1_300_000);
    }
}
