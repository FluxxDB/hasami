//! Command-line argument parsing into actions (Task 2).
//!
//! Explorer launches `hasami.exe <verb> <files…>`. [`parse_args`] turns the arguments
//! (without argv[0]) into an [`Action`].

use std::path::PathBuf;

use thiserror::Error;

/// Output format for a convert action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Mp4,
    Mov,
    Mp3,
}

/// What the process was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Trim(Vec<PathBuf>),
    Compress(Vec<PathBuf>),
    Convert(Target, Vec<PathBuf>),
    Register,
    Unregister,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CliError {
    #[error("no action given")]
    MissingVerb,
    #[error("unknown action `{0}`")]
    UnknownVerb(String),
    #[error("`{0}` needs at least one file")]
    NoFiles(String),
    #[error("`{0}` takes no files")]
    UnexpectedFiles(String),
}

/// Parse the arguments that follow the program name. `args` excludes argv[0].
pub fn parse_args(args: &[String]) -> Result<Action, CliError> {
    let (verb, rest) = args.split_first().ok_or(CliError::MissingVerb)?;

    let file_verb = |verb: &str| -> Result<Vec<PathBuf>, CliError> {
        if rest.is_empty() {
            return Err(CliError::NoFiles(verb.to_string()));
        }
        Ok(rest.iter().map(PathBuf::from).collect())
    };

    match verb.as_str() {
        "trim" => Ok(Action::Trim(file_verb("trim")?)),
        "compress" => Ok(Action::Compress(file_verb("compress")?)),
        "convert-mp4" => Ok(Action::Convert(Target::Mp4, file_verb("convert-mp4")?)),
        "convert-mov" => Ok(Action::Convert(Target::Mov, file_verb("convert-mov")?)),
        "convert-mp3" => Ok(Action::Convert(Target::Mp3, file_verb("convert-mp3")?)),
        "register" | "unregister" => {
            if !rest.is_empty() {
                return Err(CliError::UnexpectedFiles(verb.to_string()));
            }
            Ok(if verb == "register" {
                Action::Register
            } else {
                Action::Unregister
            })
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
        assert_eq!(
            action,
            Action::Compress(vec![
                PathBuf::from("C:\\a b\\x.mp4"),
                PathBuf::from("C:\\y.mkv")
            ])
        );
    }

    #[test]
    fn parse_convert_mp3() {
        let action = parse_args(&args(&["convert-mp3", "C:\\y.mkv"])).unwrap();
        assert_eq!(
            action,
            Action::Convert(Target::Mp3, vec![PathBuf::from("C:\\y.mkv")])
        );
    }

    #[test]
    fn parse_convert_mp4_and_mov() {
        let mp4 = parse_args(&args(&["convert-mp4", "a.webm"])).unwrap();
        assert_eq!(
            mp4,
            Action::Convert(Target::Mp4, vec![PathBuf::from("a.webm")])
        );
        let mov = parse_args(&args(&["convert-mov", "a.mkv"])).unwrap();
        assert_eq!(
            mov,
            Action::Convert(Target::Mov, vec![PathBuf::from("a.mkv")])
        );
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
        assert_eq!(
            parse_args(&args(&["unregister"])).unwrap(),
            Action::Unregister
        );
    }

    #[test]
    fn unknown_verb_errors() {
        assert_eq!(
            parse_args(&args(&["explode"])),
            Err(CliError::UnknownVerb("explode".to_string()))
        );
    }

    #[test]
    fn file_verb_without_files_errors() {
        assert_eq!(
            parse_args(&args(&["compress"])),
            Err(CliError::NoFiles("compress".to_string()))
        );
    }

    #[test]
    fn no_arguments_errors() {
        assert_eq!(parse_args(&[]), Err(CliError::MissingVerb));
    }

    #[test]
    fn register_with_files_errors() {
        assert_eq!(
            parse_args(&args(&["register", "a.mp4"])),
            Err(CliError::UnexpectedFiles("register".to_string()))
        );
    }
}
