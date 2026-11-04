//! Command-line argument parsing into actions.
//!
//! Explorer launches `hasami.exe <verb> <files…>`. [`parse_args`] turns the arguments
//! (without argv[0]) into an [`Action`].

use std::ffi::OsStr;
use std::path::PathBuf;

use thiserror::Error;

/// Output format for a convert action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `convert-mp4`
    Mp4,
    /// `convert-mov`
    Mov,
    /// `convert-mp3`: the audio alone.
    Mp3,
}

/// What the process was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `trim <files…>`: open the files in the Quick Trim editor.
    Trim(Vec<PathBuf>),
    /// `compress <files…>`: make a 20 MB copy of each.
    Compress(Vec<PathBuf>),
    /// `convert-<ext> <files…>`: replace each with the target format.
    Convert(Target, Vec<PathBuf>),
    /// `register`: add the right-click menu (run by the installer).
    Register,
    /// `unregister`: remove the right-click menu (run by the uninstaller).
    Unregister,
}

/// Why the arguments don't make an [`Action`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CliError {
    /// No arguments at all (started from the Start menu).
    #[error("no action given")]
    MissingVerb,
    /// The first argument isn't a known verb.
    #[error("unknown action `{0}`")]
    UnknownVerb(String),
    /// A file verb with no files after it.
    #[error("`{0}` needs at least one file")]
    NoFiles(String),
    /// `register` or `unregister` followed by files.
    #[error("`{0}` takes no files")]
    UnexpectedFiles(String),
}

/// Parse the arguments that follow the program name. `args` excludes argv[0]. File names are taken
/// as given, so a name that isn't valid Unicode (Windows allows that) still reaches the job.
pub fn parse_args<S: AsRef<OsStr>>(args: &[S]) -> Result<Action, CliError> {
    let (verb, rest) = args.split_first().ok_or(CliError::MissingVerb)?;
    let verb = verb.as_ref().to_string_lossy();

    let file_verb = |verb: &str| -> Result<Vec<PathBuf>, CliError> {
        if rest.is_empty() {
            return Err(CliError::NoFiles(verb.to_string()));
        }
        Ok(rest.iter().map(|file| PathBuf::from(file.as_ref())).collect())
    };

    match verb.as_ref() {
        "trim" => Ok(Action::Trim(file_verb("trim")?)),
        "compress" => Ok(Action::Compress(file_verb("compress")?)),
        "convert-mp4" => Ok(Action::Convert(Target::Mp4, file_verb("convert-mp4")?)),
        "convert-mov" => Ok(Action::Convert(Target::Mov, file_verb("convert-mov")?)),
        "convert-mp3" => Ok(Action::Convert(Target::Mp3, file_verb("convert-mp3")?)),
        "register" | "unregister" => {
            if !rest.is_empty() {
                return Err(CliError::UnexpectedFiles(verb.into_owned()));
            }
            Ok(if verb == "register" { Action::Register } else { Action::Unregister })
        }
        other => Err(CliError::UnknownVerb(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_compress_multiple() {
        let action = parse_args(&args(&["compress", "C:\\a b\\x.mp4", "C:\\y.mkv"])).unwrap();
        assert_eq!(action, Action::Compress(vec![PathBuf::from("C:\\a b\\x.mp4"), PathBuf::from("C:\\y.mkv")]));
    }

    #[test]
    fn parse_convert_mp3() {
        let action = parse_args(&args(&["convert-mp3", "C:\\y.mkv"])).unwrap();
        assert_eq!(action, Action::Convert(Target::Mp3, vec![PathBuf::from("C:\\y.mkv")]));
    }

    #[test]
    fn parse_convert_mp4_and_mov() {
        let mp4 = parse_args(&args(&["convert-mp4", "a.webm"])).unwrap();
        assert_eq!(mp4, Action::Convert(Target::Mp4, vec![PathBuf::from("a.webm")]));
        let mov = parse_args(&args(&["convert-mov", "a.mkv"])).unwrap();
        assert_eq!(mov, Action::Convert(Target::Mov, vec![PathBuf::from("a.mkv")]));
    }

    #[test]
    fn parse_trim() {
        let action = parse_args(&args(&["trim", "clip.mp4"])).unwrap();
        assert_eq!(action, Action::Trim(vec![PathBuf::from("clip.mp4")]));
    }

    #[test]
    fn parse_register() {
        assert_eq!(parse_args(&args(&["register"])).unwrap(), Action::Register);
    }

    #[test]
    fn parse_unregister() {
        assert_eq!(parse_args(&args(&["unregister"])).unwrap(), Action::Unregister);
    }

    #[test]
    fn unknown_verb_errors() {
        assert_eq!(parse_args(&args(&["explode"])), Err(CliError::UnknownVerb("explode".to_string())));
    }

    #[test]
    fn file_verb_without_files_errors() {
        assert_eq!(parse_args(&args(&["compress"])), Err(CliError::NoFiles("compress".to_string())));
    }

    #[test]
    fn no_arguments_errors() {
        assert_eq!(parse_args::<&str>(&[]), Err(CliError::MissingVerb));
    }

    #[test]
    fn a_file_name_that_isnt_valid_unicode_is_kept_as_is() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        // "a\u{D800}.mp4": Windows allows an unpaired surrogate in a file name.
        let odd = OsString::from_wide(&[0x61, 0xD800, 0x2E, 0x6D, 0x70, 0x34]);
        let action = parse_args(&[OsString::from("trim"), odd.clone()]).unwrap();
        assert_eq!(action, Action::Trim(vec![PathBuf::from(odd)]));
    }

    #[test]
    fn register_with_files_errors() {
        assert_eq!(parse_args(&args(&["register", "a.mp4"])), Err(CliError::UnexpectedFiles("register".to_string())));
    }
}
