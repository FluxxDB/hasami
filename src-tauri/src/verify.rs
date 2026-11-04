//! Post-encode output verification (Task 5).

use std::path::Path;

use crate::media::{probe, FfmpegPaths, MediaError, MediaInfo};

/// Why an encoded output was rejected.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The output could not be probed (missing, truncated or unreadable).
    #[error("the output file could not be read back: {0}")]
    Probe(#[from] MediaError),

    /// The output's duration is outside the allowed tolerance.
    #[error("the output is {actual_s:.2} s long but {expected_s:.2} s was expected")]
    WrongDuration { expected_s: f64, actual_s: f64 },

    /// The output is bigger than allowed.
    #[error("the output is {size_bytes} bytes, over the {max_bytes} byte limit")]
    TooLarge { size_bytes: u64, max_bytes: u64 },
}

/// Probe `file` and check that its duration is within `max(0.25 s, 1% of expected)` of
/// `expected_duration_s`, and that its size is at most `max_bytes` (if given).
pub fn verify_output(
    paths: &FfmpegPaths,
    file: &Path,
    expected_duration_s: f64,
    max_bytes: Option<u64>,
) -> Result<MediaInfo, VerifyError> {
    let info = probe(paths, file)?;
    let tolerance = (expected_duration_s * 0.01).max(0.25);
    let off_by = (info.duration_s - expected_duration_s).abs();
    // A NaN duration on either side never passes.
    if off_by.is_nan() || off_by > tolerance {
        return Err(VerifyError::WrongDuration { expected_s: expected_duration_s, actual_s: info.duration_s });
    }
    if let Some(max_bytes) = max_bytes {
        if info.size_bytes > max_bytes {
            return Err(VerifyError::TooLarge { size_bytes: info.size_bytes, max_bytes });
        }
    }
    Ok(info)
}
