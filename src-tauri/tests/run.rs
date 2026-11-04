//! Integration tests for the ffmpeg runner: progress, cancellation and failure reporting (Task 6).

mod common;

use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use hasami_lib::media::run::{run_ffmpeg, CancelToken, RunError};
use hasami_lib::media::FfmpegPaths;

fn transcode_args(input: &Path, output: &Path, preset: &str) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-i".into()];
    a.push(input.into());
    a.extend(["-c:v", "libx264", "-preset", preset, "-c:a", "aac"].iter().map(OsString::from));
    a.push(output.into());
    a
}

#[test]
fn run_reports_monotonic_progress_to_one() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let output = dir.path().join("out.mp4");
    let paths = FfmpegPaths::resolve().unwrap();

    let mut seen: Vec<f64> = Vec::new();
    run_ffmpeg(&paths, &transcode_args(&input, &output, "ultrafast"), 5.0, &CancelToken::new(), &mut |p| seen.push(p))
        .expect("transcode should succeed");

    assert!(!seen.is_empty(), "no progress reported");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "not monotonic: {seen:?}");
    assert!(seen.iter().all(|p| (0.0..=1.0).contains(p)), "out of range: {seen:?}");
    assert!(*seen.last().unwrap() >= 0.99, "last progress {:?}", seen.last());
    assert!(output.metadata().unwrap().len() > 0);
}

#[test]
fn run_cancel_kills() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_big_h264(dir.path(), 20);
    let output = dir.path().join("out.mp4");
    let paths = FfmpegPaths::resolve().unwrap();

    let cancel = CancelToken::new();
    let mut cancelled_at: Option<Instant> = None;
    let result = run_ffmpeg(&paths, &transcode_args(&input, &output, "medium"), 20.0, &cancel, &mut |_| {
        if cancelled_at.is_none() {
            cancelled_at = Some(Instant::now());
            cancel.cancel();
        }
    });
    let started = cancelled_at.expect("no progress callback before the run ended");

    assert!(matches!(result, Err(RunError::Cancelled)), "got {result:?}");
    assert!(started.elapsed() < Duration::from_secs(2), "took {:?} to return after cancel", started.elapsed());

    // The child must be dead and reaped: the output file stops growing.
    let size = output.metadata().map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(500));
    let later = output.metadata().map(|m| m.len()).unwrap_or(0);
    assert_eq!(size, later, "output kept growing after cancel; child survived");
}

#[test]
fn run_failure_has_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does-not-exist.mp4");
    let output = dir.path().join("out.mp4");
    let paths = FfmpegPaths::resolve().unwrap();

    let result =
        run_ffmpeg(&paths, &transcode_args(&missing, &output, "ultrafast"), 5.0, &CancelToken::new(), &mut |_| {});
    match result {
        Err(RunError::Failed { stderr_tail }) => {
            assert!(!stderr_tail.trim().is_empty(), "empty stderr_tail");
            assert!(stderr_tail.lines().count() <= 20);
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn run_progress_comes_from_out_time_not_just_end_marker() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::fixture_multitrack_hevc(dir.path());
    let output = dir.path().join("out.mp4");
    let paths = FfmpegPaths::resolve().unwrap();

    // Claim the clip is twice as long as it is: parsed out_time values top out near 0.5, so a
    // value around 0.5 can only come from out_time_us parsing; 1.0 then comes from progress=end.
    let mut seen: Vec<f64> = Vec::new();
    run_ffmpeg(&paths, &transcode_args(&input, &output, "ultrafast"), 10.0, &CancelToken::new(), &mut |p| seen.push(p))
        .expect("transcode should succeed");

    let parsed_max = seen.iter().copied().filter(|p| *p < 1.0).fold(0.0, f64::max);
    assert!(
        (0.45..=0.55).contains(&parsed_max),
        "expected parsed out_time near 0.5 before the end marker, got {seen:?}"
    );
    assert_eq!(seen.last(), Some(&1.0), "progress=end should finish at 1.0: {seen:?}");
}

#[test]
fn a_cancelled_token_starts_no_run() {
    let cancel = CancelToken::new();
    assert!(cancel.start_run().is_some());
    cancel.cancel();
    assert!(cancel.start_run().is_none());
}

#[test]
fn waiting_for_runs_waits_until_each_has_ended() {
    let cancel = CancelToken::new();
    // Nothing running: no wait.
    assert!(cancel.wait_for_runs(Instant::now()));

    let run = cancel.start_run().unwrap();
    let ends = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        drop(run);
        Instant::now()
    });
    cancel.cancel();
    assert!(cancel.wait_for_runs(Instant::now() + Duration::from_secs(5)));
    let returned = Instant::now();
    assert!(returned >= ends.join().unwrap(), "returned before the run ended");
}

#[test]
fn waiting_for_runs_gives_up_at_the_deadline() {
    let cancel = CancelToken::new();
    let _run = cancel.start_run().unwrap();
    let start = Instant::now();
    assert!(!cancel.wait_for_runs(start + Duration::from_millis(100)));
    assert!(start.elapsed() < Duration::from_secs(2));
}
