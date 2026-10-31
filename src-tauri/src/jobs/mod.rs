//! Job types, job runners and the serial queue.

pub mod compress;
pub mod convert;
pub mod queue;

use std::path::PathBuf;

use crate::compress::{CompressError, CompressPlan};
use crate::fsops::FsError;
use crate::media::run::RunError;
use crate::media::MediaError;
use crate::verify::VerifyError;

/// Shown when the input can't be probed (unreadable, not media, or no duration).
pub(crate) const UNREADABLE: &str = "Hasami couldn't read this video.";

/// Progress of one job, reported to the queue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JobProgress {
    pub pass: u8,
    pub passes: u8,
    pub fraction: f64,
}

/// How a job ended when it didn't fail.
#[derive(Debug, Clone, PartialEq)]
pub enum JobOutcome {
    Done { output: PathBuf, bytes: u64 },
    Unchanged { reason: String },
    NeedsConfirm { plan: CompressPlan },
}

/// Why a job failed. `Display` is shown to the user in the progress window.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("Cancelled")]
    Cancelled,
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Fs(#[from] FsError),
    #[error(transparent)]
    Media(#[from] MediaError),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error(transparent)]
    Compress(#[from] CompressError),
    /// ffmpeg failed or couldn't be started. `message` is for the user, `detail` (ffmpeg's last
    /// stderr lines or the I/O error) for debugging.
    #[error("{message}")]
    Encode { message: &'static str, detail: String },
}

impl JobError {
    /// False when the failure left the original somewhere other than its own path.
    pub fn original_intact(&self) -> bool {
        !matches!(self, JobError::Fs(e) if !e.original_intact)
    }

    /// True when the finished new file was kept (see [`FsError::kept_new_file`]); the job must
    /// not delete its temp file.
    pub fn keeps_new_file(&self) -> bool {
        matches!(self, JobError::Fs(e) if e.kept_new_file.is_some())
    }
}

/// ffmpeg's own wording for a full disk and for a folder it may not write to.
const DISK_FULL: &str = "Couldn't save the new file because the disk is full.";
const NO_WRITE_ACCESS: &str = "Couldn't write the new file in this folder.";
const PROCESS_FAILED: &str = "Couldn't process this video.";
const TOOLS_WONT_START: &str = "Couldn't start Hasami's video tools. Reinstall Hasami.";

impl From<RunError> for JobError {
    fn from(e: RunError) -> Self {
        match e {
            RunError::Cancelled => JobError::Cancelled,
            RunError::Failed { stderr_tail } => {
                let message = if stderr_tail.contains("No space left on device") {
                    DISK_FULL
                } else if stderr_tail.contains("Permission denied") {
                    NO_WRITE_ACCESS
                } else {
                    PROCESS_FAILED
                };
                JobError::Encode { message, detail: stderr_tail }
            }
            RunError::Io(err) => JobError::Encode { message: TOOLS_WONT_START, detail: err.to_string() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(stderr: &str) -> String {
        JobError::from(RunError::Failed { stderr_tail: stderr.to_owned() }).to_string()
    }

    #[test]
    fn ffmpeg_failures_read_as_plain_language() {
        assert_eq!(
            failed("[out#0/mp4 @ 0x1] Error writing trailer: No space left on device"),
            DISK_FULL
        );
        assert_eq!(failed("C:\\clips\\.a.mp4.hasami-tmp: Permission denied"), NO_WRITE_ACCESS);
        assert_eq!(failed("Invalid data found when processing input"), PROCESS_FAILED);
        assert_eq!(failed(""), PROCESS_FAILED);
    }

    #[test]
    fn ffmpeg_that_wont_start_says_to_reinstall() {
        let e = JobError::from(RunError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)));
        assert_eq!(e.to_string(), TOOLS_WONT_START);
    }

    #[test]
    fn a_cancelled_run_is_a_cancelled_job() {
        assert!(matches!(JobError::from(RunError::Cancelled), JobError::Cancelled));
    }

    #[test]
    fn only_a_stranded_commit_reports_the_original_as_moved() {
        assert!(JobError::Message("x".into()).original_intact());
        assert!(JobError::Fs(FsError::new("x")).original_intact());
        let stranded = FsError::stranded(
            std::path::Path::new(r"C:\d\.a.mp4.hasami-tmp"),
            std::io::Error::from_raw_os_error(5),
        );
        let e = JobError::Fs(stranded);
        assert!(!e.original_intact());
        assert!(e.keeps_new_file());
    }
}
