//! Finding a fixed black border around the picture (some OBS setups record the game in part of the
//! frame), so video exports can crop it off.

use std::ffi::OsString;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::media::{os, run_checked};
use crate::jobs::JobError;
use crate::media::run::CancelToken;
use crate::media::{probe, FfmpegPaths};

/// Short stretches of the clip sampled for a black border, spread evenly across it.
const BORDER_WINDOWS: usize = 10;

/// Seconds decoded per border window, from the keyframe at or before its start.
const BORDER_WINDOW_S: f64 = 0.25;

/// `cropdetect`: anything at most 24/255 bright counts as black, the result is rounded to even
/// sizes, and it reports the union of every frame it has seen. The limit is given as a fraction
/// so ffmpeg scales it to the source's bit depth: an absolute 24 is below 10-bit black (64).
const CROPDETECT: &str = "cropdetect=limit=0.0941:round=2:reset=0";

/// Decoder threads per border window, so ten windows at once don't each start a full set.
const BORDER_WINDOW_THREADS: &str = "2";

/// A rectangle of the picture in source pixels: `w`×`h` with its top-left corner at `x`,`y`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The picture area of `input` when it sits inside a fixed black border: `cropdetect` runs on
/// [`BORDER_WINDOWS`] short stretches spread across the clip, and the area counts only when every
/// stretch agrees on it, it is smaller than the frame, and it covers at least a quarter of it.
/// `None` for no border (or no video). Stops (killing ffmpeg) when `cancel` is set.
pub fn detect_border(
    paths: &FfmpegPaths,
    input: &Path,
    duration_s: f64,
    cancel: &CancelToken,
) -> Result<Option<Crop>, JobError> {
    if cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    let info = probe(paths, input)?;
    match &info.video {
        Some(v) => find_border(paths, input, (v.width, v.height), duration_s, cancel),
        None => Ok(None),
    }
}

/// [`detect_border`] for a video whose displayed frame is `frame` (width, height).
pub(super) fn find_border(
    paths: &FfmpegPaths,
    input: &Path,
    frame: (u32, u32),
    duration_s: f64,
    cancel: &CancelToken,
) -> Result<Option<Crop>, JobError> {
    let args = border_args(input, duration_s);
    let stderr = run_checked(paths, "Looking for a black border", &args, cancel)?;
    let found = window_crops(&String::from_utf8_lossy(&stderr), BORDER_WINDOWS);
    Ok(agreed_border(&found, frame))
}

/// One ffmpeg run over every border window: each is its own input, fast-seeked to the keyframe
/// at or before its start (so nothing before it is decoded) and read for [`BORDER_WINDOW_S`],
/// with its own `cropdetect` (named `Parsed_cropdetect_<window>` in the log).
fn border_args(input: &Path, duration_s: f64) -> Vec<OsString> {
    let duration_s = if duration_s.is_finite() && duration_s > 0.0 { duration_s } else { 0.0 };
    let mut args = Vec::new();
    let mut chains = Vec::new();
    for i in 0..BORDER_WINDOWS {
        let start = duration_s * (i as f64 + 0.5) / BORDER_WINDOWS as f64;
        args.extend([os("-threads"), os(BORDER_WINDOW_THREADS)]);
        args.extend([os("-ss"), os(&format!("{start:.3}")), os("-noaccurate_seek")]);
        args.extend([os("-t"), os(&BORDER_WINDOW_S.to_string()), os("-i"), input.as_os_str().to_os_string()]);
        chains.push(format!("[{i}:v:0]{CROPDETECT}[w{i}]"));
    }
    args.extend([os("-filter_complex"), os(&chains.join(";"))]);
    for i in 0..BORDER_WINDOWS {
        args.extend([os("-map"), os(&format!("[w{i}]"))]);
    }
    args.extend([os("-f"), os("null"), os("-")]);
    args
}

/// What each window's `cropdetect` settled on: its last `crop=w:h:x:y`, as `(x, y, w, h)` (the
/// size can come out negative for an all-black stretch). `None` for a window that reported nothing.
fn window_crops(log: &str, windows: usize) -> Vec<Option<(i64, i64, i64, i64)>> {
    let mut found = vec![None; windows];
    for line in log.lines() {
        let Some(rest) = line.strip_prefix("[Parsed_cropdetect_") else { continue };
        let Some(window) = rest.split(' ').next().and_then(|n| n.parse::<usize>().ok()) else { continue };
        let Some((_, crop)) = line.rsplit_once("crop=") else { continue };
        let n: Vec<i64> = crop.trim().split(':').filter_map(|v| v.parse().ok()).collect();
        if let (&[w, h, x, y], Some(slot)) = (n.as_slice(), found.get_mut(window)) {
            *slot = Some((x, y, w, h));
        }
    }
    found
}

/// The border every window agrees on, if it is a real one: inside `frame`, smaller than it and at
/// least a quarter of its area. An all-black window (a fade or a loading screen, reported with a
/// non-positive size) agrees with any border, but at least one window must show the picture; a
/// window that reported nothing at all disagrees.
fn agreed_border(found: &[Option<(i64, i64, i64, i64)>], frame: (u32, u32)) -> Option<Crop> {
    let all_black = |&(_, _, w, h): &(i64, i64, i64, i64)| w <= 0 || h <= 0;
    let mut seen = found.iter().copied().collect::<Option<Vec<_>>>()?;
    seen.retain(|f| !all_black(f));
    let first = *seen.first()?;
    if seen.iter().any(|f| *f != first) {
        return None;
    }
    let (x, y, w, h) = first;
    let (fw, fh) = (i64::from(frame.0), i64::from(frame.1));
    let inside = x >= 0 && y >= 0 && w > 0 && h > 0 && x + w <= fw && y + h <= fh;
    if !inside || w * h == fw * fh || w * h * 4 < fw * fh {
        return None;
    }
    Some(Crop { x: x as u32, y: y as u32, w: w as u32, h: h as u32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_line(window: usize, crop: &str) -> String {
        format!(
            "[Parsed_cropdetect_{window} @ 000001d2] x1:0 x2:1439 y1:0 y2:809 w:1440 h:810 x:0 y:0 \
             pts:0 t:0.000000 limit:0.094118 crop={crop}\n"
        )
    }

    #[test]
    fn each_window_keeps_its_last_crop() {
        let mut log = String::from("Input #0, mov,mp4 ...\n");
        log += &window_line(0, "1920:1080:0:0");
        log += &window_line(1, "1440:810:0:0");
        log += &window_line(0, "1440:810:0:0");
        log += &window_line(2, "-1918:-1078:1920:1080");
        log += "[Parsed_cropdetect_9 @ 000001d2] something else\n";
        let found = window_crops(&log, 4);
        assert_eq!(found, [Some((0, 0, 1440, 810)), Some((0, 0, 1440, 810)), Some((1920, 1080, -1918, -1078)), None]);
        // A window number past the count is ignored rather than trusted.
        assert_eq!(window_crops(&window_line(12, "2:2:0:0"), 4), [None; 4]);
    }

    #[test]
    fn a_border_counts_only_when_every_window_agrees() {
        const FRAME: (u32, u32) = (1920, 1080);
        let picture = Some((0, 0, 1440, 810));
        assert_eq!(agreed_border(&[picture; 10], FRAME), Some(Crop { x: 0, y: 0, w: 1440, h: 810 }));

        let mut one_off = [picture; 10];
        one_off[7] = Some((0, 0, 1440, 808));
        assert_eq!(agreed_border(&one_off, FRAME), None, "one stretch disagrees");
        let mut silent = [picture; 10];
        silent[3] = None;
        assert_eq!(agreed_border(&silent, FRAME), None, "one stretch reported nothing");
        assert_eq!(agreed_border(&[], FRAME), None);

        // A fade to black in one stretch doesn't hide the border; an all-black clip has none.
        let black = Some((1920, 1080, -1918, -1078));
        let mut faded = [picture; 10];
        faded[0] = black;
        assert_eq!(agreed_border(&faded, FRAME), Some(Crop { x: 0, y: 0, w: 1440, h: 810 }));
        assert_eq!(agreed_border(&[black; 10], FRAME), None);
    }

    #[test]
    fn a_border_must_be_smaller_than_the_frame_and_a_quarter_of_it() {
        const FRAME: (u32, u32) = (1920, 1080);
        assert_eq!(agreed_border(&[Some((0, 0, 1920, 1080)); 3], FRAME), None, "no border at all");
        // Exactly a quarter still counts; any less doesn't.
        let quarter = Some(Crop { x: 480, y: 270, w: 960, h: 540 });
        assert_eq!(agreed_border(&[Some((480, 270, 960, 540)); 3], FRAME), quarter);
        assert_eq!(agreed_border(&[Some((480, 270, 958, 540)); 3], FRAME), None);
        // Numbers that don't fit the frame.
        assert_eq!(agreed_border(&[Some((600, 0, 1440, 810)); 3], FRAME), None);
    }

    #[test]
    fn border_windows_are_spread_across_the_clip() {
        let text: Vec<String> =
            border_args(Path::new("in.mp4"), 100.0).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let starts: Vec<&str> = text.windows(2).filter(|w| w[0] == "-ss").map(|w| w[1].as_str()).collect();
        let want: Vec<String> = (0..10).map(|i| format!("{}.000", 5 + 10 * i)).collect();
        assert_eq!(starts, want);
        let graph = &text[text.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(graph.starts_with("[0:v:0]cropdetect=limit=0.0941:round=2:reset=0[w0];[1:v:0]"), "{graph}");
        assert!(graph.ends_with("[9:v:0]cropdetect=limit=0.0941:round=2:reset=0[w9]"), "{graph}");
        assert_eq!(text.windows(2).filter(|w| w[0] == "-threads" && w[1] == "2").count(), 10);
        assert_eq!(text.iter().filter(|a| *a == "-map").count(), 10);
    }
}
