//! ffmpeg/ffprobe location, probing and process running.

pub mod ffmpeg;
pub mod probe;
pub mod run;

use std::path::PathBuf;

pub use ffmpeg::{hidden_command, FfmpegPaths};
pub use probe::{probe, AudioStream, MediaInfo, VideoStream};

/// Errors from locating or running the bundled media tools.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// `ffmpeg`/`ffprobe` were not found in any of the searched directories.
    #[error("ffmpeg binaries not found (searched: {})", .searched.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    BinariesNotFound { searched: Vec<PathBuf> },

    /// Spawning or talking to a child process failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// ffprobe exited with a failure status.
    #[error("ffprobe failed for {}: {stderr}", .file.display())]
    ProbeFailed { file: PathBuf, stderr: String },

    /// ffprobe output could not be understood.
    #[error("could not parse ffprobe output: {0}")]
    ProbeParse(String),
}
