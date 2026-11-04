//! The Quick Trim editor's backend (the page itself lives in `src/editor/`).
//!
//! - [`media`]: preparing a clip: audio tracks, thumbnails and the preview proxy.
//! - [`border`]: finding a fixed black border around the picture.
//! - [`peaks`]: waveform peaks of an audio track.
//! - [`session`]: one work folder per open clip, and closing it cleanly.
//! - [`folder`]: the neighbouring videos for Prev / Next, and ordering a selection by date.
//! - [`request`]: the export request, the ffmpeg filter graph built from it, and the encoder choice.

pub mod border;
pub mod folder;
pub mod media;
pub mod peaks;
pub mod request;
pub mod session;
