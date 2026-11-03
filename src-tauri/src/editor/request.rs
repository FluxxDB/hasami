//! The export request the editor sends to Rust, the ffmpeg `-filter_complex` graph built from it,
//! and the same-codec video encoder choice (AMD AMF hardware when available, else software).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

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
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

/// Build the filter graph for `req`: video pieces trimmed and concatenated into `[vout]`; each
/// audio track's pieces trimmed, delayed to their place on the timeline, mixed and gained into
/// `[t<k>]`; all tracks mixed, given the master gain, padded with silence and cut to exactly the
/// output length (kept audio may end before the video does), and made stereo into `[aout]`.
pub fn build_graph(req: &ExportRequest) -> Graph {
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
        if req.video.len() == 1 {
            parts.push("[v0]null[vout]".to_owned());
        } else {
            let labels: String = (0..req.video.len()).map(|i| format!("[v{i}]")).collect();
            parts.push(format!("{labels}concat=n={}:v=1:a=0[vout]", req.video.len()));
        }
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
        let master = if req.master_db != 0.0 {
            format!("volume={:.2}dB,", req.master_db)
        } else {
            String::new()
        };
        let dur = num(req.duration_s);
        parts.push(format!(
            "{}{mix}{master}apad=whole_dur={dur},atrim=end={dur},aformat=channel_layouts=stereo[aout]",
            track_labels.concat()
        ));
        Some("[aout]".to_owned())
    };

    Graph { filter: parts.join(";"), video_out, audio_out }
}

/// A video encoder name for `-c:v` and the rate/format arguments that go with it.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoEncoder {
    pub codec: &'static str,
    pub args: Vec<String>,
}

/// Whether the bundled ffmpeg can encode with `name` on this machine: a one-frame test encode,
/// remembered per encoder name for the life of the process.
pub fn encoder_available(paths: &FfmpegPaths, name: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(&known) = cache.lock().unwrap().get(name) {
        return known;
    }
    // Not holding the lock while ffmpeg runs; two threads racing just both run the test.
    let ok = hidden_command(&paths.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=640x360:r=30:d=1"])
        .args(["-frames:v", "1", "-pix_fmt", "yuv420p", "-c:v", name, "-f", "null", "-"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    cache.lock().unwrap().insert(name.to_owned(), ok);
    ok
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// Bitrate arguments shared by the AMF and software H.264/HEVC encoders: average `b`, peak 1.5×.
fn rate_args(b: u64) -> Vec<String> {
    vec!["-b:v".into(), b.to_string(), "-maxrate".into(), (b + b / 2).to_string()]
}

/// Pick an encoder that keeps the source's codec at about `bitrate` bits/s. `hw` says whether a
/// named hardware encoder works here (normally [`encoder_available`]). Codecs without a rule
/// here (e.g. ProRes) are encoded as H.264, or as VP9 in WebM, which can't hold H.264.
pub fn same_codec_encoder(
    source_codec: &str,
    container: &str,
    bitrate: u64,
    hw: &dyn Fn(&str) -> bool,
) -> VideoEncoder {
    let b = bitrate;
    match source_codec {
        "hevc" => {
            let (codec, mut a) = if hw("hevc_amf") {
                ("hevc_amf", [args(&["-rc", "vbr_peak"]), rate_args(b)].concat())
            } else {
                (
                    "libx265",
                    [args(&["-preset", "medium"]), rate_args(b), args(&["-bufsize", &(b * 2).to_string(), "-pix_fmt", "yuv420p"])]
                        .concat(),
                )
            };
            if matches!(container, "mp4" | "mov") {
                a.extend(args(&["-tag:v", "hvc1"]));
            }
            VideoEncoder { codec, args: a }
        }
        "av1" => {
            if hw("av1_amf") {
                VideoEncoder { codec: "av1_amf", args: args(&["-b:v", &b.to_string()]) }
            } else {
                VideoEncoder { codec: "libsvtav1", args: args(&["-b:v", &b.to_string()]) }
            }
        }
        "vp9" => VideoEncoder { codec: "libvpx-vp9", args: args(&["-b:v", &b.to_string()]) },
        "vp8" if matches!(container, "webm" | "mkv") => VideoEncoder { codec: "libvpx", args: args(&["-b:v", &b.to_string()]) },
        _ if container == "webm" => VideoEncoder { codec: "libvpx-vp9", args: args(&["-b:v", &b.to_string()]) },
        // h264 and anything else.
        _ => {
            if hw("h264_amf") {
                VideoEncoder { codec: "h264_amf", args: [args(&["-rc", "vbr_peak"]), rate_args(b)].concat() }
            } else {
                VideoEncoder {
                    codec: "libx264",
                    args: [args(&["-preset", "medium"]), rate_args(b), args(&["-bufsize", &(b * 2).to_string(), "-pix_fmt", "yuv420p"])]
                        .concat(),
                }
            }
        }
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

/// Audio encoder arguments for `container`: Opus for WebM, AAC otherwise.
pub fn audio_codec_args(container: &str) -> Vec<String> {
    if container == "webm" {
        args(&["-c:a", "libopus", "-b:a", "160k"])
    } else {
        args(&["-c:a", "aac", "-b:a", "192k"])
    }
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
        }
    }

    fn vp(src_start: f64, src_end: f64) -> VideoPiece {
        VideoPiece { src_start, src_end }
    }

    fn ap(at: f64, src_start: f64, src_end: f64) -> AudioPiece {
        AudioPiece { at, src_start, src_end }
    }

    #[test]
    fn graph_single_piece_no_audio() {
        let g = build_graph(&req(vec![vp(2.0, 5.0)], vec![], 3.0));
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
        let g = build_graph(&req(vec![vp(0.0, 12.0), vp(31.0, 90.0)], audio, 71.0));
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
        let f = build_graph(&louder).filter;
        assert!(
            f.ends_with(
                "[t0][t1]amix=inputs=2:normalize=0:duration=longest,volume=6.00dB,\
                 apad=whole_dur=3,atrim=end=3,aformat=channel_layouts=stereo[aout]"
            ),
            "{f}"
        );

        let single = ExportRequest { master_db: -2.5, ..req(vec![vp(0.0, 3.0)], vec![track(0)], 3.0) };
        let f = build_graph(&single).filter;
        assert!(f.ends_with("[t0]volume=-2.50dB,apad=whole_dur=3,atrim=end=3,aformat=channel_layouts=stereo[aout]"), "{f}");

        let unity = build_graph(&req(vec![vp(0.0, 3.0)], vec![track(0), track(1)], 3.0)).filter;
        assert_eq!(unity.matches("volume=").count(), 2, "only the two track gains: {unity}");
        assert!(unity.contains("duration=longest,apad="), "{unity}");
    }

    #[test]
    fn master_db_defaults_to_zero() {
        let json = r#"{"session":1,"input":"in.mp4","mode":"quick","fit_20mb":false,"duration_s":1,"video":[],"audio":[]}"#;
        let req: ExportRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.master_db, 0.0);
    }

    #[test]
    fn graph_audio_mode_has_no_video() {
        let audio = vec![AudioTrackRequest { index: 0, gain_db: 0.0, pieces: vec![ap(0.0, 1.0, 4.0)] }];
        let g = build_graph(&req(vec![], audio, 3.0));
        assert_eq!(g.video_out, None);
        assert_eq!(g.audio_out.as_deref(), Some("[aout]"));
        assert!(!g.filter.contains("[0:v:0]"), "{}", g.filter);
    }

    #[test]
    fn graph_numbers_are_trimmed_not_rounded_away() {
        let g = build_graph(&req(vec![vp(1.5, 2.123_456_789)], vec![], 0.62));
        assert!(g.filter.contains("trim=start=1.5:end=2.123457"), "{}", g.filter);
    }

    #[test]
    fn encoder_hevc_hw_and_tag() {
        let e = same_codec_encoder("hevc", "mp4", 10_000_000, &|_| true);
        assert_eq!(e.codec, "hevc_amf");
        assert!(e.args.windows(2).any(|w| w == ["-tag:v", "hvc1"]), "{:?}", e.args);
        assert!(e.args.windows(2).any(|w| w == ["-b:v", "10000000"]), "{:?}", e.args);
        assert!(e.args.windows(2).any(|w| w == ["-maxrate", "15000000"]), "{:?}", e.args);

        let sw = same_codec_encoder("hevc", "mp4", 10_000_000, &|_| false);
        assert_eq!(sw.codec, "libx265");
    }

    #[test]
    fn encoder_hevc_in_mkv_has_no_hvc1_tag() {
        let e = same_codec_encoder("hevc", "mkv", 10_000_000, &|_| false);
        assert!(!e.args.iter().any(|a| a == "-tag:v"), "{:?}", e.args);
    }

    #[test]
    fn encoder_unknown_falls_back_to_h264() {
        let e = same_codec_encoder("prores", "mov", 10_000_000, &|_| false);
        assert_eq!(e.codec, "libx264");
        assert!(e.args.windows(2).any(|w| w == ["-bufsize", "20000000"]), "{:?}", e.args);
    }

    #[test]
    fn encoder_av1_and_vp9() {
        assert_eq!(same_codec_encoder("av1", "mp4", 5_000_000, &|n| n == "av1_amf").codec, "av1_amf");
        assert_eq!(same_codec_encoder("av1", "mp4", 5_000_000, &|_| false).codec, "libsvtav1");
        assert_eq!(same_codec_encoder("vp9", "webm", 5_000_000, &|_| true).codec, "libvpx-vp9");
    }

    #[test]
    fn encoder_never_puts_h264_in_webm() {
        // VP8 keeps its codec in containers that take it; WebM takes no H.264 at all.
        let vp8 = same_codec_encoder("vp8", "webm", 5_000_000, &|_| true);
        assert_eq!(vp8.codec, "libvpx");
        assert_eq!(vp8.args, ["-b:v", "5000000"]);
        assert_eq!(same_codec_encoder("vp8", "mkv", 5_000_000, &|_| true).codec, "libvpx");
        assert_eq!(same_codec_encoder("theora", "webm", 5_000_000, &|_| true).codec, "libvpx-vp9");
        assert_eq!(same_codec_encoder("vp8", "mp4", 5_000_000, &|_| false).codec, "libx264");
    }

    fn info(video_bps: Option<u64>, total_bps: Option<u64>, tracks: usize) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from("in.mp4"),
            container: "mp4".into(),
            duration_s: 10.0,
            size_bytes: 1,
            bit_rate: total_bps,
            video: Some(VideoStream {
                index: 0,
                codec: "h264".into(),
                width: 1920,
                height: 1080,
                fps: 30.0,
                bit_rate: video_bps,
            }),
            audio: (0..tracks)
                .map(|i| AudioStream { index: i + 1, codec: "aac".into(), channels: 2, sample_rate: 48000, bit_rate: None })
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
    fn audio_args_follow_container() {
        assert_eq!(audio_codec_args("webm"), ["-c:a", "libopus", "-b:a", "160k"]);
        assert_eq!(audio_codec_args("mp4"), ["-c:a", "aac", "-b:a", "192k"]);
    }
}
