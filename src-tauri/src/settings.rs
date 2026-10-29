//! Persistent settings (D8, Settings → General).
//!
//! The settings live in `settings.json` inside the folder Tauri resolves with
//! `app.path().app_config_dir()`. A missing file means the defaults. An unreadable or invalid
//! file also means the defaults, with the reason on stderr: a damaged file never blocks the app.
//! A single bad field (an unknown theme, for example) falls back to its own default and keeps
//! the others. Writes are atomic: a temporary file in the same folder, then `rename`.
//!
//! Starting with the system is not a setting stored here: the operating system owns it (see
//! `autostart`).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime, State};

/// The name of the settings file inside the app's config folder.
pub const SETTINGS_FILE: &str = "settings.json";

/// The interface theme: `system` follows the desktop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// The settings as saved and as the frontend receives them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub theme: Theme,
    /// Start minimised to the tray when launched by the autostart entry.
    pub start_minimised: bool,
}

/// The managed state: where the file lives and the settings last loaded or saved.
pub struct SettingsState {
    /// `None` when Tauri could not resolve the config folder: the defaults apply and saving
    /// reports an error.
    file: Option<PathBuf>,
    current: Mutex<Settings>,
}

/// Loads the settings and registers them as managed state. Never fails: every problem is
/// reported on stderr and the defaults apply.
pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let file = match app.path().app_config_dir() {
        Ok(dir) => Some(dir.join(SETTINGS_FILE)),
        Err(error) => {
            eprintln!("redline: settings folder not available, using the defaults: {error}");
            None
        }
    };
    let settings = match &file {
        Some(file) => {
            let (settings, problems) = load(file);
            for problem in problems {
                eprintln!("redline: {problem}");
            }
            settings
        }
        None => Settings::default(),
    };
    app.manage(SettingsState {
        file,
        current: Mutex::new(settings),
    });
}

/// The current settings; the defaults when no file has been saved yet.
#[tauri::command]
pub fn get_settings(state: State<'_, SettingsState>) -> Result<Settings, String> {
    state
        .current
        .lock()
        .map(|current| *current)
        .map_err(|_| "Settings are not available: restart the app.".to_owned())
}

/// Saves the settings and returns them as read back from the file.
#[tauri::command]
pub fn set_settings(
    state: State<'_, SettingsState>,
    settings: Settings,
) -> Result<Settings, String> {
    let mut current = state
        .current
        .lock()
        .map_err(|_| "Settings are not available: restart the app.".to_owned())?;
    let file = state
        .file
        .as_deref()
        .ok_or_else(|| "Could not save the settings: no settings folder.".to_owned())?;
    let saved = save(file, &settings)?;
    *current = saved;
    Ok(saved)
}

/// Reads the settings file. Returns the settings to use and the problems found, already worded
/// for stderr; a missing file is not a problem.
fn load(file: &Path) -> (Settings, Vec<String>) {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return (Settings::default(), Vec::new());
        }
        Err(error) => {
            return (
                Settings::default(),
                vec![format!(
                    "could not read {SETTINGS_FILE} ({}), using the defaults",
                    error.kind()
                )],
            );
        }
    };
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(&text) else {
        return (
            Settings::default(),
            vec![format!(
                "{SETTINGS_FILE} is not a valid JSON object, using the defaults"
            )],
        );
    };

    let mut problems = Vec::new();
    let mut settings = Settings::default();
    if let Some(value) = fields.get("theme") {
        match Theme::deserialize(value) {
            Ok(theme) => settings.theme = theme,
            Err(_) => problems.push(format!(
                "{SETTINGS_FILE}: unknown theme, using \"system\" (allowed: system, light, dark)"
            )),
        }
    }
    if let Some(value) = fields.get("start_minimised") {
        match value.as_bool() {
            Some(start_minimised) => settings.start_minimised = start_minimised,
            None => problems.push(format!(
                "{SETTINGS_FILE}: start_minimised is not true or false, using false"
            )),
        }
    }
    (settings, problems)
}

/// Writes the settings atomically and reads them back.
fn save(file: &Path, settings: &Settings) -> Result<Settings, String> {
    write_atomically(file, settings)
        .map_err(|error| format!("Could not save the settings ({}).", error.kind()))?;
    let (saved, problems) = load(file);
    match problems.first() {
        None => Ok(saved),
        Some(problem) => Err(format!(
            "The settings were saved but not read back: {problem}."
        )),
    }
}

fn write_atomically(file: &Path, settings: &Settings) -> io::Result<()> {
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let mut json = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
    json.push(b'\n');
    let tmp = file.with_extension("json.tmp");
    let written = (|| {
        let mut out = fs::File::create(&tmp)?;
        out.write_all(&json)?;
        out.sync_all()?;
        fs::rename(&tmp, file)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the system temp directory, never the real config folder.
    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("redline-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_gives_the_defaults_without_problems() {
        let dir = temp_dir("missing");
        let (settings, problems) = load(&dir.join(SETTINGS_FILE));
        assert_eq!(
            settings,
            Settings {
                theme: Theme::System,
                start_minimised: false,
            }
        );
        assert!(problems.is_empty());
    }

    #[test]
    fn invalid_json_gives_the_defaults_and_a_problem() {
        let dir = temp_dir("invalid");
        let file = dir.join(SETTINGS_FILE);
        for text in ["{ not json", "[]", "\"dark\"", ""] {
            fs::write(&file, text).unwrap();
            let (settings, problems) = load(&file);
            assert_eq!(settings, Settings::default(), "{text:?}");
            assert_eq!(problems.len(), 1, "{text:?}");
        }
    }

    #[test]
    fn an_unreadable_file_gives_the_defaults_and_a_problem() {
        let dir = temp_dir("unreadable");
        // A folder where the file should be: reading it fails with something other than
        // "not found".
        let file = dir.join(SETTINGS_FILE);
        fs::create_dir_all(&file).unwrap();
        let (settings, problems) = load(&file);
        assert_eq!(settings, Settings::default());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn an_unknown_theme_falls_back_to_system_and_keeps_the_other_fields() {
        let dir = temp_dir("unknown-theme");
        let file = dir.join(SETTINGS_FILE);
        fs::write(&file, r#"{"theme": "purple", "start_minimised": true}"#).unwrap();
        let (settings, problems) = load(&file);
        assert_eq!(
            settings,
            Settings {
                theme: Theme::System,
                start_minimised: true,
            }
        );
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("unknown theme"));
    }

    #[test]
    fn a_wrong_type_falls_back_per_field_and_missing_fields_are_silent() {
        let dir = temp_dir("fields");
        let file = dir.join(SETTINGS_FILE);
        fs::write(&file, r#"{"theme": "dark", "start_minimised": "yes"}"#).unwrap();
        let (settings, problems) = load(&file);
        assert_eq!(settings.theme, Theme::Dark);
        assert!(!settings.start_minimised);
        assert_eq!(problems.len(), 1);

        fs::write(&file, r#"{"theme": "light", "future_option": 3}"#).unwrap();
        let (settings, problems) = load(&file);
        assert_eq!(settings.theme, Theme::Light);
        assert!(!settings.start_minimised);
        assert!(problems.is_empty());
    }

    #[test]
    fn a_round_trip_creates_the_folder_and_reads_back_what_was_saved() {
        let dir = temp_dir("round-trip");
        let file = dir.join("nested/config").join(SETTINGS_FILE);
        for wanted in [
            Settings {
                theme: Theme::Dark,
                start_minimised: true,
            },
            Settings {
                theme: Theme::Light,
                start_minimised: false,
            },
            Settings::default(),
        ] {
            assert_eq!(save(&file, &wanted), Ok(wanted));
            assert_eq!(load(&file), (wanted, Vec::new()));
        }
        // The temporary file does not survive a successful write.
        assert!(!file.with_extension("json.tmp").exists());
    }

    #[test]
    fn saving_replaces_a_damaged_file() {
        let dir = temp_dir("replace-damaged");
        let file = dir.join(SETTINGS_FILE);
        fs::write(&file, "{ damaged").unwrap();
        let wanted = Settings {
            theme: Theme::Dark,
            start_minimised: false,
        };
        assert_eq!(save(&file, &wanted), Ok(wanted));
    }

    #[test]
    fn the_file_uses_the_documented_names_and_values() {
        let dir = temp_dir("shape");
        let file = dir.join(SETTINGS_FILE);
        let settings = Settings {
            theme: Theme::Light,
            start_minimised: true,
        };
        save(&file, &settings).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"theme": "light", "start_minimised": true})
        );
    }

    #[test]
    fn a_failed_write_is_an_error_and_touches_nothing_else() {
        let dir = temp_dir("failed-write");
        // The parent "folder" is a regular file: creating the folder fails.
        let blocker = dir.join("blocker");
        fs::write(&blocker, "").unwrap();
        let file = blocker.join(SETTINGS_FILE);
        assert!(save(&file, &Settings::default()).is_err());
        assert_eq!(fs::read_to_string(&blocker).unwrap(), "");
    }
}
