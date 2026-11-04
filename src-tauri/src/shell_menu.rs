//! The Explorer right-click menu: its registry layout, and registering or removing it.
//!
//! Writes a cascading "Hasami" submenu under `HKCU\<classes_root>\SystemFileAssociations\.<ext>\shell`
//! for each supported video extension. `register` and `unregister` are called from the NSIS
//! install/uninstall hooks, so they run without starting the UI.

use std::io;
use std::path::{Path, PathBuf};

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

/// Errors from writing or removing the context menu registry keys.
#[derive(Debug, thiserror::Error)]
pub enum MenuError {
    /// Reading or writing a registry key failed.
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
    Verb { key: "01trim", label: "Quick Trim", command: "trim", skip_for: None },
    Verb { key: "02compress", label: "Compress", command: "compress", skip_for: None },
    Verb { key: "03mp4", label: "Convert to .mp4", command: "convert-mp4", skip_for: Some("mp4") },
    Verb { key: "04mov", label: "Convert to .mov", command: "convert-mov", skip_for: Some("mov") },
    Verb { key: "05mp3", label: "Convert to .mp3", command: "convert-mp3", skip_for: None },
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
        // Start clean so verbs from an older layout don't survive an upgrade.
        delete_menu(&hkcu, classes_root, ext)?;
        let (menu, _) = hkcu.create_subkey(menu_path(classes_root, ext))?;
        menu.set_value("MUIVerb", &"Hasami")?;
        menu.set_value("SubCommands", &"")?;
        menu.set_value("Icon", &format!("{exe},0"))?;
        // Explorer hides a cascade whose parent has the default "Document" model once more than
        // 15 files are selected, so the parent needs "Player" as well as each verb.
        menu.set_value("MultiSelectModel", &"Player")?;

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

/// Runs `register`/`unregister` for the matching CLI action and returns the process exit code
/// (0 on success, 1 on failure). Returns `None` for any other action so the caller starts the app.
/// A failure is reported on stderr; the installer only looks at the exit code.
pub fn handle_action(action: &crate::cli::Action, current_exe: io::Result<PathBuf>, classes_root: &str) -> Option<i32> {
    use crate::cli::Action;
    let result = match action {
        Action::Register => current_exe.map_err(MenuError::from).and_then(|exe| register(&exe, classes_root)),
        Action::Unregister => unregister(classes_root),
        _ => return None,
    };
    Some(match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("hasami: {e}");
            1
        }
    })
}

/// Removes the Hasami cascade for every supported video extension. Missing keys are not an error.
pub fn unregister(classes_root: &str) -> Result<(), MenuError> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for ext in EXTENSIONS {
        delete_menu(&hkcu, classes_root, ext)?;
    }
    Ok(())
}

/// Deletes one extension's `shell\Hasami` tree. A missing key is not an error.
fn delete_menu(hkcu: &RegKey, classes_root: &str, ext: &str) -> Result<(), MenuError> {
    match hkcu.delete_subkey_all(menu_path(classes_root, ext)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
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
            let lock = TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        open(&format!(r"{}\shell\{verb}", menu_path(TEST_CLASSES_ROOT, ext))).is_ok()
    }

    fn command_of(ext: &str, verb: &str) -> String {
        let path = format!(r"{}\shell\{verb}\command", menu_path(TEST_CLASSES_ROOT, ext));
        open(&path).and_then(|key| key.get_value::<String, _>("")).expect("command default value should be readable")
    }

    #[test]
    fn register_writes_cascade() {
        let _guard = TestGuard::new();
        register(Path::new(EXE), TEST_CLASSES_ROOT).unwrap();

        let menu = hasami_key("mkv");
        assert_eq!(menu.get_value::<String, _>("MUIVerb").unwrap(), "Hasami");
        assert_eq!(menu.get_value::<String, _>("SubCommands").unwrap(), "");
        assert_eq!(
            menu.get_value::<String, _>("MultiSelectModel").unwrap(),
            "Player",
            "the cascade itself must allow large selections"
        );
        assert_eq!(menu.get_value::<String, _>("Icon").unwrap(), format!(r"{EXE},0"));

        let expected = [
            ("01trim", "Quick Trim", "trim"),
            ("02compress", "Compress", "compress"),
            ("03mp4", "Convert to .mp4", "convert-mp4"),
            ("04mov", "Convert to .mov", "convert-mov"),
            ("05mp3", "Convert to .mp3", "convert-mp3"),
        ];
        for (verb, label, command) in expected {
            let key = open(&format!(r"{}\shell\{verb}", menu_path(TEST_CLASSES_ROOT, "mkv")))
                .unwrap_or_else(|_| panic!("mkv is missing verb {verb}"));
            assert_eq!(key.get_value::<String, _>("MUIVerb").unwrap(), label);
            assert_eq!(key.get_value::<String, _>("MultiSelectModel").unwrap(), "Player", "{verb} MultiSelectModel");
            assert_eq!(command_of("mkv", verb), format!(r#""{EXE}" {command} "%1""#), "{verb} command");
        }
    }

    #[test]
    fn register_removes_stale_verbs_from_an_older_layout() {
        let _guard = TestGuard::new();
        register(Path::new(EXE), TEST_CLASSES_ROOT).unwrap();

        // Pretend an older version registered a verb this version no longer has.
        let stale = format!(r"{}\shell\00old\command", menu_path(TEST_CLASSES_ROOT, "mkv"));
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&stale).unwrap();
        key.set_value("", &"old.exe").unwrap();
        assert!(has_verb("mkv", "00old"));

        register(Path::new(EXE), TEST_CLASSES_ROOT).unwrap();
        assert!(!has_verb("mkv", "00old"), "stale verb survived re-register");
        assert!(has_verb("mkv", "02compress"));
    }

    #[test]
    fn menu_actions_map_to_exit_codes() {
        use crate::cli::Action;
        let _guard = TestGuard::new();
        let exe = || Ok(PathBuf::from(EXE));

        assert_eq!(handle_action(&Action::Register, exe(), TEST_CLASSES_ROOT), Some(0));
        assert!(has_verb("mkv", "02compress"));
        assert_eq!(handle_action(&Action::Unregister, exe(), TEST_CLASSES_ROOT), Some(0));
        assert!(!has_verb("mkv", "02compress"));

        // Failure to locate the exe is a nonzero exit, not a panic.
        let err = Err(io::Error::other("no exe"));
        assert_eq!(handle_action(&Action::Register, err, TEST_CLASSES_ROOT), Some(1));

        // Anything else is not a menu action; the caller starts the app.
        assert_eq!(handle_action(&Action::Compress(vec![]), exe(), TEST_CLASSES_ROOT), None);
    }

    #[test]
    fn mp4_menu_omits_convert_mp4() {
        let _guard = TestGuard::new();
        register(Path::new(EXE), TEST_CLASSES_ROOT).unwrap();

        assert!(!has_verb("mp4", "03mp4"), "mp4 menu must not offer convert-mp4");
        assert!(has_verb("mp4", "04mov"));

        assert!(!has_verb("mov", "04mov"), "mov menu must not offer convert-mov");
        assert!(has_verb("mov", "03mp4"));
    }

    #[test]
    fn unregister_removes_all_and_is_idempotent() {
        let _guard = TestGuard::new();
        register(Path::new(EXE), TEST_CLASSES_ROOT).unwrap();

        unregister(TEST_CLASSES_ROOT).unwrap();
        for ext in ["mp4", "mov", "mkv", "webm"] {
            assert!(open(&menu_path(TEST_CLASSES_ROOT, ext)).is_err(), "{ext} menu should be gone after unregister");
        }

        unregister(TEST_CLASSES_ROOT).unwrap();
    }
}
