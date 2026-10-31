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
    #[error("The video couldn't be processed. {0}")]
    Encode(String),
}

impl JobError {
    /// True when the finished new file was kept (see [`FsError::kept_new_file`]); the job must
    /// not delete its temp file.
    pub fn keeps_new_file(&self) -> bool {
        matches!(self, JobError::Fs(e) if e.kept_new_file.is_some())
    }
}

impl From<RunError> for JobError {
    fn from(e: RunError) -> Self {
        match e {
            RunError::Cancelled => JobError::Cancelled,
            RunError::Failed { stderr_tail } => JobError::Encode(
                stderr_tail.lines().last().unwrap_or("ffmpeg failed.").to_string(),
            ),
            other => JobError::Encode(other.to_string()),
        }
    }
}
