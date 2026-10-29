//! Starting with the system (D8).
//!
//! `tauri-plugin-autostart` writes the operating system's entry: on Linux an XDG autostart
//! `.desktop` file in the user's autostart folder. The entry launches the app with
//! [`AUTOSTART_ARG`], so the app can tell an autostart launch from a manual one.
//!
//! The operating system is the only source of truth: the state is never copied into the
//! settings file, and every write is followed by a read, so an entry removed by hand is not
//! reported as active.
//!
//! The main window is declared invisible in `tauri.conf.json` (no flash at startup):
//! [`show_main_window_unless_hidden_start`] shows it in `setup`, unless [`start_hidden`] says
//! that an autostart launch stays in the tray.

use std::ffi::OsStr;

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_autostart::{AutoLaunchManager, ManagerExt};

use crate::settings::{self, SettingsState};
use crate::tray;

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

/// Whether the app starts with the window hidden and only the tray icon: only when the
/// autostart entry launched it (`args` without the program name), `start_minimised` was read and
/// is on, and something shows the tray icon right now. `tray_available` is asked last: it queries
/// the session bus, and a manual launch must not wait for it.
///
/// It is asked once, with no retries: if the shell has not registered its tray host yet, the
/// window opens. An extra window closes with one click; a hidden window with no icon could not
/// be reached, and retrying for a few seconds would leave the app unreachable during exactly
/// those seconds, with no guarantee that the shell ever arrives.
pub fn start_hidden<I, S>(
    args: I,
    start_minimised: Option<bool>,
    tray_available: impl FnOnce() -> bool,
) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let autostarted = args
        .into_iter()
        .any(|arg| arg.as_ref() == OsStr::new(AUTOSTART_ARG));
    autostarted && start_minimised == Some(true) && tray_available()
}

/// Shows the main window, invisible at startup, unless [`start_hidden`] says to stay in the
/// tray. Called in `setup` after `settings::init` and `tray::create`.
pub fn show_main_window_unless_hidden_start<R: Runtime>(app: &AppHandle<R>) {
    // `None` when the settings are not readable: the window opens.
    let start_minimised = app
        .try_state::<SettingsState>()
        .and_then(|state| settings::get_settings(state).ok())
        .map(|settings| settings.start_minimised);
    let hidden = start_hidden(std::env::args_os().skip(1), start_minimised, || {
        tray::tray_available(app)
    });
    if !hidden {
        tray::show_main_window(app);
    }
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

    #[test]
    fn only_an_autostart_launch_with_start_minimised_and_a_tray_icon_starts_hidden() {
        let autostart = [AUTOSTART_ARG];
        let manual: [&str; 0] = [];
        for (args, start_minimised, tray, hidden) in [
            (&autostart[..], Some(true), true, true),
            // Start minimised off or unreadable: the window.
            (&autostart[..], Some(false), true, false),
            (&autostart[..], None, true, false),
            // No visible tray icon: the window, or the app could not be reached.
            (&autostart[..], Some(true), false, false),
            (&autostart[..], Some(false), false, false),
            (&autostart[..], None, false, false),
            // Launched by hand: always the window.
            (&manual[..], Some(true), true, false),
            (&manual[..], Some(true), false, false),
            (&manual[..], Some(false), true, false),
            (&manual[..], Some(false), false, false),
            (&manual[..], None, true, false),
            (&manual[..], None, false, false),
        ] {
            assert_eq!(
                start_hidden(args, start_minimised, || tray),
                hidden,
                "{args:?} {start_minimised:?} {tray}"
            );
        }
    }

    #[test]
    fn the_argument_is_found_among_others_and_only_as_a_whole_word() {
        assert!(start_hidden(["--x", AUTOSTART_ARG], Some(true), || true));
        assert!(!start_hidden(["--autostart=1"], Some(true), || true));
        assert!(!start_hidden(["--AUTOSTART"], Some(true), || true));
        assert!(!start_hidden(["--other"], Some(true), || true));
    }

    #[test]
    fn the_tray_is_asked_only_when_it_decides() {
        let asked = Cell::new(0);
        let probe = || {
            asked.set(asked.get() + 1);
            true
        };
        let manual: [&str; 0] = [];
        assert!(!start_hidden(manual, Some(true), probe));
        assert!(!start_hidden([AUTOSTART_ARG], Some(false), probe));
        assert!(!start_hidden([AUTOSTART_ARG], None, probe));
        assert_eq!(asked.get(), 0);
        assert!(start_hidden([AUTOSTART_ARG], Some(true), probe));
        assert_eq!(asked.get(), 1);
    }

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
