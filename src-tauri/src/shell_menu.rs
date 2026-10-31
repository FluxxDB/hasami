//! Explorer context menu registry layout (Task 10).
//!
//! Writes a cascading "Hasami" submenu under `HKCU\<classes_root>\SystemFileAssociations\.<ext>\shell`
//! for each supported video extension. `register` and `unregister` are called from the NSIS
//! install/uninstall hooks, so they run without starting the UI.

use std::io;
use std::path::Path;

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

/// Errors from writing or removing the context menu registry keys.
#[derive(Debug, thiserror::Error)]
pub enum MenuError {
    #[error("registry operation failed: {0}")]
    Registry(#[from] std::io::Error),
}

/// Registry root used by the installer. Tests pass a throwaway root instead.
pub const DEFAULT_CLASSES_ROOT: &str = r"Software\Classes";

/// Video extensions that get the Hasami cascade.
const EXTENSIONS: [&str; 4] = ["mp4", "mov", "mkv", "webm"];

/// One entry in the Hasami submenu. `skip_for` omits a convert verb from its own container type.
struct Verb {
    key: &'static str,
    label: &'static str,
    command: &'static str,
    skip_for: Option<&'static str>,
}

const VERBS: [Verb; 5] = [
    Verb {
        key: "01trim",
        label: "Quick Trim",
        command: "trim",
        skip_for: None,
    },
    Verb {
        key: "02compress",
        label: "Compress",
        command: "compress",
        skip_for: None,
    },
    Verb {
        key: "03mp4",
        label: "Convert to .mp4",
        command: "convert-mp4",
        skip_for: Some("mp4"),
    },
    Verb {
        key: "04mov",
        label: "Convert to .mov",
        command: "convert-mov",
        skip_for: Some("mov"),
    },
    Verb {
        key: "05mp3",
        label: "Convert to .mp3",
        command: "convert-mp3",
        skip_for: None,
    },
];

/// Path of the `shell\Hasami` key for one extension, relative to HKCU.
fn menu_path(classes_root: &str, ext: &str) -> String {
    format!(r"{classes_root}\SystemFileAssociations\.{ext}\shell\Hasami")
}

/// Registers the Hasami cascade for every supported video extension.
pub fn register(exe: &Path, classes_root: &str) -> Result<(), MenuError> {
    let exe = exe.to_string_lossy();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for ext in EXTENSIONS {
        let (menu, _) = hkcu.create_subkey(menu_path(classes_root, ext))?;
        menu.set_value("MUIVerb", &"Hasami")?;
        menu.set_value("SubCommands", &"")?;
        menu.set_value("Icon", &format!("{exe},0"))?;

        let (shell, _) = menu.create_subkey("shell")?;
        for verb in VERBS.iter().filter(|v| v.skip_for != Some(ext)) {
            let (key, _) = shell.create_subkey(verb.key)?;
            key.set_value("MUIVerb", &verb.label)?;
            key.set_value("MultiSelectModel", &"Player")?;
            let (command, _) = key.create_subkey("command")?;
            command.set_value("", &format!(r#""{exe}" {} "%1""#, verb.command))?;
        }
    }
    Ok(())
}

/// Removes the Hasami cascade for every supported video extension. Missing keys are not an error.
pub fn unregister(classes_root: &str) -> Result<(), MenuError> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for ext in EXTENSIONS {
        match hkcu.delete_subkey_all(menu_path(classes_root, ext)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    const TEST_CLASSES_ROOT: &str = r"Software\HasamiTest\Classes";
    const TEST_HIVE: &str = r"Software\HasamiTest";
    const EXE: &str = r"C:\x\hasami.exe";

    /// The registry tests share one throwaway tree, so they must not run concurrently.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Holds the test lock and wipes `HKCU\Software\HasamiTest` on creation and on drop,
    /// including when the test panics.
    struct TestGuard {
        _lock: MutexGuard<'static, ()>,
    }

    impl TestGuard {
        fn new() -> Self {
            let lock = TEST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            wipe_test_hive().expect("could not clear HKCU\\Software\\HasamiTest before the test");
            TestGuard { _lock: lock }
        }
    }

    impl Drop for TestGuard {
        fn drop(&mut self) {
            let _ = wipe_test_hive();
        }
    }

    fn wipe_test_hive() -> io::Result<()> {
        match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(TEST_HIVE) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Opens `HKCU\<path>` read-only.
    fn open(path: &str) -> io::Result<RegKey> {
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(path)
    }

    /// Opens the `shell\Hasami` key for an extension in the test root.
    fn hasami_key(ext: &str) -> RegKey {
        open(&menu_path(TEST_CLASSES_ROOT, ext)).expect("Hasami key should exist")
    }

    fn has_verb(ext: &str, verb: &str) -> bool {
        open(&format!(
            r"{}\shell\{verb}",
            menu_path(TEST_CLASSES_ROOT, ext)
        ))
        .is_ok()
    }

    fn command_of(ext: &str, verb: &str) -> String {
        let path = format!(
            r"{}\shell\{verb}\command",
            menu_path(TEST_CLASSES_ROOT, ext)
        );
        open(&path)
            .and_then(|key| key.get_value::<String, _>(""))
            .expect("command default value should be readable")
    }

    #[test]
    fn register_writes_cascade() {
        let _guard = TestGuard::new();
        register(Path::new(EXE), TEST_CLASSES_ROOT).unwrap();

        let menu = hasami_key("mkv");
        assert_eq!(menu.get_value::<String, _>("MUIVerb").unwrap(), "Hasami");
        assert_eq!(menu.get_value::<String, _>("SubCommands").unwrap(), "");
        assert_eq!(
            menu.get_value::<String, _>("Icon").unwrap(),
            format!(r"{EXE},0")
        );

        for verb in ["01trim", "02compress", "03mp4", "04mov", "05mp3"] {
            assert!(has_verb("mkv", verb), "mkv is missing verb {verb}");
        }

        assert_eq!(
            command_of("mkv", "02compress"),
            r#""C:\x\hasami.exe" compress "%1""#
        );
    }

    #[test]
    fn mp4_menu_omits_convert_mp4() {
        let _guard = TestGuard::new();
        register(Path::new(EXE), TEST_CLASSES_ROOT).unwrap();

        assert!(
            !has_verb("mp4", "03mp4"),
            "mp4 menu must not offer convert-mp4"
        );
        assert!(has_verb("mp4", "04mov"));

        assert!(
            !has_verb("mov", "04mov"),
            "mov menu must not offer convert-mov"
        );
        assert!(has_verb("mov", "03mp4"));
    }

    #[test]
    fn unregister_removes_all_and_is_idempotent() {
        let _guard = TestGuard::new();
        register(Path::new(EXE), TEST_CLASSES_ROOT).unwrap();

        unregister(TEST_CLASSES_ROOT).unwrap();
        for ext in ["mp4", "mov", "mkv", "webm"] {
            assert!(
                open(&menu_path(TEST_CLASSES_ROOT, ext)).is_err(),
                "{ext} menu should be gone after unregister"
            );
        }

        unregister(TEST_CLASSES_ROOT).unwrap();
    }
}
