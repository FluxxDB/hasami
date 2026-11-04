//! ffprobe output parsed into [`MediaInfo`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{hidden_command, FfmpegPaths, MediaError};

/// A file's video stream, as ffprobe reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VideoStream {
    pub index: usize,
    pub codec: String,
    /// Display size: the coded size with width and height swapped for a 90° or 270° rotation,
    /// matching the frames ffmpeg's autorotate hands to filters.
    pub width: u32,
    pub height: u32,
    /// From `avg_frame_rate`.
    pub fps: f64,
    pub bit_rate: Option<u64>,
}

/// One of a file's audio streams, as ffprobe reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AudioStream {
    pub index: usize,
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub bit_rate: Option<u64>,
}

/// What ffprobe says about a file. Stream `index`es are ffmpeg's absolute `0:<index>`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MediaInfo {
    pub path: PathBuf,
    /// File extension, lowercased, without the dot (empty if there is none).
    pub container: String,
    pub duration_s: f64,
    pub size_bytes: u64,
    pub bit_rate: Option<u64>,
    /// The first non-cover-art video stream.
    pub video: Option<VideoStream>,
    pub audio: Vec<AudioStream>,
}

/// Run ffprobe on `file` and summarize its streams.
pub fn probe(paths: &FfmpegPaths, file: &Path) -> Result<MediaInfo, MediaError> {
    let output = hidden_command(&paths.ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", "-i"])
        .arg(file)
        .output()?;
    if !output.status.success() {
        return Err(MediaError::ProbeFailed {
            file: file.to_path_buf(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    parse(file, &output.stdout)
}

#[derive(Deserialize)]
struct RawProbe {
    #[serde(default)]
    streams: Vec<RawStream>,
    format: RawFormat,
}

#[derive(Deserialize)]
struct RawFormat {
    duration: Option<String>,
    size: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Deserialize)]
struct RawStream {
    index: usize,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    channels: Option<u32>,
    sample_rate: Option<String>,
    bit_rate: Option<String>,
    disposition: Option<RawDisposition>,
    #[serde(default)]
    side_data_list: Vec<RawSideData>,
    tags: Option<RawTags>,
}

#[derive(Deserialize)]
struct RawSideData {
    /// Degrees, from a "Display Matrix" entry, e.g. `-90`.
    rotation: Option<f64>,
}

#[derive(Deserialize)]
struct RawTags {
    /// Older files and ffmpeg versions: degrees as a string, e.g. `"90"`.
    rotate: Option<String>,
}

impl RawStream {
    /// Whether the picture is displayed turned a quarter (90° or 270°, either direction).
    fn quarter_turned(&self) -> bool {
        let rotation = self
            .side_data_list
            .iter()
            .find_map(|d| d.rotation)
            .or_else(|| self.tags.as_ref()?.rotate.as_deref()?.trim().parse().ok());
        rotation.is_some_and(|r| (r.round() as i64).rem_euclid(180) == 90)
    }
}

#[derive(Deserialize)]
struct RawDisposition {
    #[serde(default)]
    attached_pic: u8,
}

fn parse(file: &Path, json: &[u8]) -> Result<MediaInfo, MediaError> {
    let raw: RawProbe = serde_json::from_slice(json).map_err(|e| MediaError::ProbeParse(e.to_string()))?;

    let duration_s = raw
        .format
        .duration
        .as_deref()
        .and_then(|d| d.parse::<f64>().ok())
        .ok_or_else(|| MediaError::ProbeParse("missing format duration".into()))?;
    let size_bytes = match raw.format.size.as_deref().and_then(|s| s.parse().ok()) {
        Some(size) => size,
        None => std::fs::metadata(file)?.len(),
    };

    let mut video = None;
    let mut audio = Vec::new();
    for s in &raw.streams {
        let codec = s.codec_name.clone().unwrap_or_default();
        let bit_rate = s.bit_rate.as_deref().and_then(|b| b.parse().ok());
        match s.codec_type.as_deref() {
            // Cover art shows up as a one-picture "video" stream; skip it.
            Some("video") if video.is_none() && s.disposition.as_ref().is_none_or(|d| d.attached_pic != 1) => {
                let (mut width, mut height) = (s.width.unwrap_or(0), s.height.unwrap_or(0));
                if s.quarter_turned() {
                    std::mem::swap(&mut width, &mut height);
                }
                video = Some(VideoStream {
                    index: s.index,
                    codec,
                    width,
                    height,
                    fps: s.avg_frame_rate.as_deref().map(parse_rate).unwrap_or(0.0),
                    bit_rate,
                });
            }
            Some("audio") => audio.push(AudioStream {
                index: s.index,
                codec,
                channels: s.channels.unwrap_or(0),
                sample_rate: s.sample_rate.as_deref().and_then(|r| r.parse().ok()).unwrap_or(0),
                bit_rate,
            }),
            _ => {}
        }
    }

    Ok(MediaInfo {
        path: file.to_path_buf(),
        container: file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default(),
        duration_s,
        size_bytes,
        bit_rate: raw.format.bit_rate.as_deref().and_then(|b| b.parse().ok()),
        video,
        audio,
    })
}

/// Parse an ffprobe rational like `60/1` or `30000/1001`; `0/0` and garbage give 0.
fn parse_rate(rate: &str) -> f64 {
    let Some((num, den)) = rate.split_once('/') else {
        return rate.parse().unwrap_or(0.0);
    };
    match (num.parse::<f64>(), den.parse::<f64>()) {
        (Ok(n), Ok(d)) if d != 0.0 => n / d,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_rate};
    use std::path::Path;

    /// ffprobe JSON for a 1920x1080 stream with the given extra stream fields.
    fn video_json(extra: &str) -> Vec<u8> {
        format!(
            r#"{{"streams":[{{"index":0,"codec_type":"video","codec_name":"h264","width":1920,"height":1080,
                "avg_frame_rate":"30/1"{extra}}}],
              "format":{{"duration":"10.0","size":"1000"}}}}"#
        )
        .into_bytes()
    }

    fn size(extra: &str) -> (u32, u32) {
        let v = parse(Path::new("x.mp4"), &video_json(extra)).unwrap().video.unwrap();
        (v.width, v.height)
    }

    #[test]
    fn a_quarter_turned_video_reports_its_display_size() {
        let matrix = |deg: &str| {
            format!(
                r#","side_data_list":[{{"side_data_type":"Display Matrix","displaymatrix":"...","rotation":{deg}}}]"#
            )
        };
        assert_eq!(size(&matrix("-90")), (1080, 1920));
        assert_eq!(size(&matrix("90")), (1080, 1920));
        assert_eq!(size(&matrix("270")), (1080, 1920));
        assert_eq!(size(&matrix("180")), (1920, 1080));
        assert_eq!(size(&matrix("0")), (1920, 1080));
        assert_eq!(size(r#","tags":{"rotate":"90"}"#), (1080, 1920));
        assert_eq!(size(r#","tags":{"language":"und"}"#), (1920, 1080));
        assert_eq!(size(""), (1920, 1080));
    }

    #[test]
    fn rates() {
        assert_eq!(parse_rate("60/1"), 60.0);
        assert!((parse_rate("30000/1001") - 29.97).abs() < 0.001);
        assert_eq!(parse_rate("0/0"), 0.0);
        assert_eq!(parse_rate("25"), 25.0);
    }
}
