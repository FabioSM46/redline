//! Starting with the system (D8).
//!
//! `tauri-plugin-autostart` writes the operating system's entry: on Linux an XDG autostart
//! `.desktop` file in the user's autostart folder. The entry launches the app with
//! [`AUTOSTART_ARG`], so the app can tell an autostart launch from a manual one.
//!
//! The operating system is the only source of truth: the state is never copied into the
//! settings file, and every write is followed by a read, so an entry removed by hand is not
//! reported as active.

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::{AutoLaunchManager, ManagerExt};

/// The argument the autostart entry launches the app with.
pub const AUTOSTART_ARG: &str = "--autostart";

/// The plugin, registered for the backend only: no `autostart:*` permission is granted to the
/// frontend, which goes through [`get_autostart`] and [`set_autostart`].
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_autostart::Builder::new()
        .arg(AUTOSTART_ARG)
        .build()
}

/// Whether the app starts with the system, read from the operating system.
#[tauri::command]
pub fn get_autostart(app: AppHandle) -> Result<bool, String> {
    read(&*app.autolaunch())
}

/// Turns starting with the system on or off and returns the state read back from the operating
/// system, not the requested one.
#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    write(&*app.autolaunch(), enabled)
}

/// The plugin's three operations on the operating system entry, behind a trait so that
/// [`read`] and [`write`] can be tested without touching the real entry.
trait Launcher {
    fn enable(&self) -> Result<(), String>;
    fn disable(&self) -> Result<(), String>;
    fn is_enabled(&self) -> Result<bool, String>;
}

impl Launcher for AutoLaunchManager {
    fn enable(&self) -> Result<(), String> {
        AutoLaunchManager::enable(self).map_err(|error| error.to_string())
    }

    fn disable(&self) -> Result<(), String> {
        AutoLaunchManager::disable(self).map_err(|error| error.to_string())
    }

    fn is_enabled(&self) -> Result<bool, String> {
        AutoLaunchManager::is_enabled(self).map_err(|error| error.to_string())
    }
}

fn read(launcher: &impl Launcher) -> Result<bool, String> {
    launcher
        .is_enabled()
        .map_err(|error| format!("Could not read whether Redline starts with the system: {error}"))
}

fn write(launcher: &impl Launcher, enabled: bool) -> Result<bool, String> {
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|error| {
        let action = if enabled { "turn on" } else { "turn off" };
        format!("Could not {action} starting with the system: {error}")
    })?;
    read(launcher)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// An entry in memory: the tests never write the real autostart entry.
    #[derive(Default)]
    struct FakeLauncher {
        enabled: Cell<bool>,
        fail_write: Option<&'static str>,
        fail_read: Option<&'static str>,
        /// The system accepts the write but the entry stays as it was.
        ignore_write: bool,
    }

    impl FakeLauncher {
        fn set(&self, enabled: bool) -> Result<(), String> {
            if let Some(error) = self.fail_write {
                return Err(error.to_owned());
            }
            if !self.ignore_write {
                self.enabled.set(enabled);
            }
            Ok(())
        }
    }

    impl Launcher for FakeLauncher {
        fn enable(&self) -> Result<(), String> {
            self.set(true)
        }

        fn disable(&self) -> Result<(), String> {
            self.set(false)
        }

        fn is_enabled(&self) -> Result<bool, String> {
            match self.fail_read {
                Some(error) => Err(error.to_owned()),
                None => Ok(self.enabled.get()),
            }
        }
    }

    #[test]
    fn a_write_returns_the_state_read_back_from_the_system() {
        let launcher = FakeLauncher::default();
        assert_eq!(read(&launcher), Ok(false));
        assert_eq!(write(&launcher, true), Ok(true));
        assert_eq!(read(&launcher), Ok(true));
        assert_eq!(write(&launcher, false), Ok(false));

        let stubborn = FakeLauncher {
            ignore_write: true,
            ..FakeLauncher::default()
        };
        assert_eq!(write(&stubborn, true), Ok(false));
    }

    #[test]
    fn errors_name_the_action_and_the_cause() {
        let failing = FakeLauncher {
            fail_write: Some("permission denied"),
            ..FakeLauncher::default()
        };
        assert_eq!(
            write(&failing, true),
            Err("Could not turn on starting with the system: permission denied".to_owned())
        );
        assert_eq!(
            write(&failing, false),
            Err("Could not turn off starting with the system: permission denied".to_owned())
        );

        let unreadable = FakeLauncher {
            fail_read: Some("no config directory"),
            ..FakeLauncher::default()
        };
        assert_eq!(
            read(&unreadable),
            Err(
                "Could not read whether Redline starts with the system: no config directory"
                    .to_owned()
            )
        );
        assert!(write(&unreadable, true).is_err());
    }
}
