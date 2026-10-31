//! Locate bundled binaries and build hidden child-process commands.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::MediaError;

/// Env var naming a directory that holds `ffmpeg.exe` and `ffprobe.exe`; overrides everything else.
pub const FFMPEG_DIR_ENV: &str = "HASAMI_FFMPEG_DIR";

/// Windows process-creation flag that stops a console window from appearing for a child process.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Locations of the bundled `ffmpeg` and `ffprobe` executables.
#[derive(Debug, Clone, PartialEq)]
pub struct FfmpegPaths {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl FfmpegPaths {
    /// Find the binaries. First match wins:
    /// 1. directory in `HASAMI_FFMPEG_DIR` (`ffmpeg.exe`, `ffprobe.exe`),
    /// 2. the current executable's directory (`ffmpeg.exe`, `ffprobe.exe`),
    /// 3. dev fallback: `<crate>/binaries/{ffmpeg,ffprobe}-x86_64-pc-windows-msvc.exe`.
    pub fn resolve() -> Result<Self, MediaError> {
        let mut searched = Vec::new();

        if let Some(dir) = std::env::var_os(FFMPEG_DIR_ENV) {
            searched.push(PathBuf::from(dir));
        }
        if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
            searched.push(dir);
        }

        for dir in &searched {
            if let Some(found) = Self::in_dir(dir, "ffmpeg.exe", "ffprobe.exe") {
                return Ok(found);
            }
        }

        let dev_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries");
        if let Some(found) = Self::in_dir(
            &dev_dir,
            "ffmpeg-x86_64-pc-windows-msvc.exe",
            "ffprobe-x86_64-pc-windows-msvc.exe",
        ) {
            return Ok(found);
        }
        searched.push(dev_dir);

        Err(MediaError::BinariesNotFound { searched })
    }

    fn in_dir(dir: &Path, ffmpeg: &str, ffprobe: &str) -> Option<Self> {
        let (ffmpeg, ffprobe) = (dir.join(ffmpeg), dir.join(ffprobe));
        (ffmpeg.is_file() && ffprobe.is_file()).then_some(Self { ffmpeg, ffprobe })
    }
}

/// A [`Command`] for `program` that does not flash a console window on Windows.
/// Every child process Hasami starts must be built through this.
pub fn hidden_command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
