//! Starting with the system (D8).
//!
//! `tauri-plugin-autostart` writes the operating system's entry: on Linux an XDG autostart
//! `.desktop` file in the user's autostart folder. The entry launches the app with
//! [`AUTOSTART_ARG`], so the app can tell an autostart launch from a manual one. The entry is
//! named after the bundle identifier ([`ENTRY_NAME`]), like the desktop file the `.deb` installs,
//! so a login start gives the portal the same app id as a menu launch; an entry an older build
//! wrote as `redline.desktop` is replaced at startup ([`repair_entries`]). The plugin also uses
//! that identifier as the entry's `Name`, which the desktop shows among the startup applications,
//! so every entry it writes is corrected to [`DISPLAY_NAME`] afterwards ([`correct_name`]).
//!
//! The operating system is the only source of truth: the state is never copied into the
//! settings file, and every write is followed by a read, so an entry removed by hand is not
//! reported as active.
//!
//! The main window is declared invisible in `tauri.conf.json` (no flash at startup):
//! [`show_main_window_unless_hidden_start`] shows it in `setup`, unless [`start_hidden`] says
//! that an autostart launch, or a launch with a command-line action, stays in the tray.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_autostart::{AutoLaunchManager, ManagerExt};

use crate::actions;
use crate::settings::{self, SettingsState};
use crate::tray;

/// The argument the autostart entry launches the app with.
pub const AUTOSTART_ARG: &str = "--autostart";

/// The entry's name, which the plugin gives the file (`<name>.desktop`) and also writes as its
/// `Name`. The plugin is built before the app's configuration is reachable, so it is written
/// here; a test ties it to the bundle identifier.
const ENTRY_NAME: &str = "com.fabiosm46.redline";

/// The `Name` the entry shows on the desktop.
const DISPLAY_NAME: &str = "Redline";

/// The file older builds wrote, named after the Linux `productName`.
const LEGACY_ENTRY: &str = "redline.desktop";

/// The plugin, registered for the backend only: no `autostart:*` permission is granted to the
/// frontend, which goes through [`get_autostart`] and [`set_autostart`].
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_autostart::Builder::new()
        .app_name(ENTRY_NAME)
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
    write_and_name(
        &*app.autolaunch(),
        autostart_folder(&app).as_deref(),
        enabled,
    )
}

/// Whether the app starts with the window hidden and only the tray icon, from `args` without the
/// program name: when something shows the tray icon right now and either
/// - the autostart entry launched it and `start_minimised` was read and is on, or
/// - the launch asks for an action (`redline --save-replay`, see `actions.rs`): a shortcut pressed
///   in a game must not open a window over it, whatever `start_minimised` says.
///
/// `tray_available` is asked last: it queries the session bus, and a manual launch must not wait
/// for it.
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
    let args: Vec<S> = args.into_iter().collect();
    let autostarted = args
        .iter()
        .any(|arg| arg.as_ref() == OsStr::new(AUTOSTART_ARG));
    let action = actions::requested(&args).is_some();
    (action || (autostarted && start_minimised == Some(true))) && tray_available()
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

/// The folder the plugin writes to: always under the home, whatever `XDG_CONFIG_HOME` says.
fn autostart_folder<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let home = app.path().home_dir().ok()?;
    Some(home.join(".config").join("autostart"))
}

/// Brings the entries in the autostart folder up to date. Called in `setup`, after the plugin.
pub fn repair_entries<R: Runtime>(app: &AppHandle<R>) {
    if let Some(folder) = autostart_folder(app) {
        repair(&folder, &*app.autolaunch());
    }
}

/// Replaces the entry an older build wrote under [`LEGACY_ENTRY`] with one under [`ENTRY_NAME`],
/// so an update neither loses starting with the system nor starts the app twice, then corrects
/// the name of the entry. Failures are only logged: the entry in place still starts the app, and
/// the next launch tries again.
fn repair(folder: &Path, launcher: &impl Launcher) {
    if let Err(error) = migrate(&folder.join(LEGACY_ENTRY), launcher) {
        eprintln!("redline: the old autostart entry was not replaced ({error})");
    }
    show_display_name(folder);
}

fn migrate(legacy: &Path, launcher: &impl Launcher) -> Result<(), String> {
    let content = match fs::read_to_string(legacy) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !written_by_redline(&content) {
        return Ok(());
    }
    // In this order: a failure leaves the old entry working instead of no entry at all.
    launcher.enable()?;
    fs::remove_file(legacy).map_err(|error| error.to_string())
}

/// Whether the file is an active entry written by Redline, which launches with
/// [`AUTOSTART_ARG`]. The name alone could be a file the user made by hand, which is not ours to
/// remove; an entry the desktop's startup settings turned off stays off, so it is left too.
fn written_by_redline(content: &str) -> bool {
    launches_with_the_argument(content)
        && !content
            .lines()
            .map(str::trim)
            .any(|line| line == "Hidden=true" || line == "X-GNOME-Autostart-enabled=false")
}

/// Whether the entry's `Exec=` launches with [`AUTOSTART_ARG`], as only Redline's entries do.
fn launches_with_the_argument(content: &str) -> bool {
    content.lines().map(str::trim).any(|line| {
        line.starts_with("Exec=") && line.split_whitespace().any(|word| word == AUTOSTART_ARG)
    })
}

/// Corrects the name of the entry in `folder`. A failure is only logged: the entry in place still
/// starts the app.
fn show_display_name(folder: &Path) {
    if let Err(error) = correct_name(&folder.join(format!("{ENTRY_NAME}.desktop"))) {
        eprintln!("redline: the autostart entry keeps its old name ({error})");
    }
}

/// Gives `entry` the [`DISPLAY_NAME`] if the plugin left the identifier there. Only an entry of
/// Redline's is rewritten, and only that line of it: `Exec=` stays as it is, and so do the keys
/// that turn an entry off, because the desktop's startup settings list a turned-off entry too and
/// it stays off. The rename is atomic, so at no moment is the entry missing or half written.
fn correct_name(entry: &Path) -> io::Result<()> {
    let content = match fs::read_to_string(entry) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !launches_with_the_argument(&content) {
        return Ok(());
    }
    let Some(corrected) = with_display_name(&content) else {
        return Ok(());
    };
    // The extension no longer ends in `.desktop`, so the desktop never takes it for an entry.
    let tmp = entry.with_extension(format!("desktop.tmp.{}", std::process::id()));
    let written = (|| {
        // Whatever a crashed run left under this name goes first, and the new file is created,
        // never opened: a link sitting there is not followed.
        let _ = fs::remove_file(&tmp);
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        out.write_all(corrected.as_bytes())?;
        out.sync_all()?;
        fs::rename(&tmp, entry)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// `content` with the `Name` line changed, or `None` if no line holds the identifier: an entry
/// already corrected, or with a name that is not the plugin's.
fn with_display_name(content: &str) -> Option<String> {
    let plugin_name = format!("Name={ENTRY_NAME}");
    let mut found = false;
    let corrected = content
        .split_inclusive('\n')
        .map(|line| {
            if line.trim() == plugin_name {
                found = true;
                line.replacen(ENTRY_NAME, DISPLAY_NAME, 1)
            } else {
                line.to_owned()
            }
        })
        .collect();
    found.then_some(corrected)
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

/// [`write`], then the name of the entry the plugin has just written. The state is read back from
/// the system as usual: a name that could not be corrected does not change it.
fn write_and_name(
    launcher: &impl Launcher,
    folder: Option<&Path>,
    enabled: bool,
) -> Result<bool, String> {
    let state = write(launcher, enabled)?;
    if let (true, Some(folder)) = (enabled, folder) {
        show_display_name(folder);
    }
    Ok(state)
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
    fn a_launch_with_an_action_starts_hidden_when_there_is_a_tray() {
        for args in [
            &["--save-replay"][..],
            &["--toggle-replay"][..],
            &["--toggle-recording"][..],
            &[AUTOSTART_ARG, "--save-replay"][..],
            &["--unknown", "--toggle-recording"][..],
        ] {
            // Whatever start_minimised says: a shortcut must not open a window over a game.
            for start_minimised in [Some(true), Some(false), None] {
                assert!(
                    start_hidden(args, start_minimised, || true),
                    "{args:?} {start_minimised:?}"
                );
                // No tray icon: the window, or the app could not be reached.
                assert!(
                    !start_hidden(args, start_minimised, || false),
                    "{args:?} {start_minimised:?}"
                );
            }
        }
        // Unknown arguments alone change nothing.
        assert!(!start_hidden(["--save-replay=1"], Some(false), || true));
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
    fn the_entry_and_the_template_are_named_after_the_bundle_identifier() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], ENTRY_NAME);
        assert_eq!(
            config["bundle"]["linux"]["deb"]["desktopTemplate"],
            format!("linux/{ENTRY_NAME}.desktop")
        );
    }

    /// A temporary folder that disappears with the test.
    struct Folder(std::path::PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("redline-autostart-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn legacy(&self) -> std::path::PathBuf {
            self.0.join(LEGACY_ENTRY)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const OLD_ENTRY: &str = "[Desktop Entry]\nType=Application\nName=redline\n\
        Exec=/usr/bin/redline --autostart\nTerminal=false";

    #[test]
    fn an_old_entry_is_replaced_not_duplicated() {
        let folder = Folder::new("replaced");
        fs::write(folder.legacy(), OLD_ENTRY).unwrap();
        let launcher = FakeLauncher::default();

        assert_eq!(migrate(&folder.legacy(), &launcher), Ok(()));
        assert!(launcher.enabled.get());
        assert!(!folder.legacy().exists());
    }

    #[test]
    fn without_an_old_entry_nothing_turns_on() {
        let folder = Folder::new("none");
        let launcher = FakeLauncher::default();

        assert_eq!(migrate(&folder.legacy(), &launcher), Ok(()));
        assert!(!launcher.enabled.get());
    }

    #[test]
    fn a_file_the_user_made_is_left_alone() {
        let folder = Folder::new("user");
        let content = "[Desktop Entry]\nName=mine\nExec=/usr/bin/redline --autostart-later";
        fs::write(folder.legacy(), content).unwrap();
        let launcher = FakeLauncher::default();

        assert_eq!(migrate(&folder.legacy(), &launcher), Ok(()));
        assert!(!launcher.enabled.get());
        assert_eq!(fs::read_to_string(folder.legacy()).unwrap(), content);
    }

    #[test]
    fn an_entry_turned_off_in_the_desktop_stays_off() {
        for key in ["Hidden=true", "X-GNOME-Autostart-enabled=false"] {
            let folder = Folder::new(key);
            let content = format!("{OLD_ENTRY}\n{key}\n");
            fs::write(folder.legacy(), &content).unwrap();
            let launcher = FakeLauncher::default();

            assert_eq!(migrate(&folder.legacy(), &launcher), Ok(()));
            assert!(!launcher.enabled.get(), "{key}");
            assert_eq!(fs::read_to_string(folder.legacy()).unwrap(), content);
        }
    }

    #[test]
    fn the_old_entry_survives_a_failed_replacement() {
        let folder = Folder::new("failed");
        fs::write(folder.legacy(), OLD_ENTRY).unwrap();
        let launcher = FakeLauncher {
            fail_write: Some("permission denied"),
            ..FakeLauncher::default()
        };

        assert_eq!(
            migrate(&folder.legacy(), &launcher),
            Err("permission denied".to_owned())
        );
        assert!(folder.legacy().exists());
    }

    /// The file as `auto-launch` 0.6.0 writes it, with no final newline.
    const PLUGIN_ENTRY: &str = "[Desktop Entry]\nType=Application\nVersion=1.0\n\
        Name=com.fabiosm46.redline\nComment=com.fabiosm46.redline startup script\n\
        Exec=/usr/bin/redline --autostart\nStartupNotify=false\nTerminal=false";

    /// The same file with only the `Name` line corrected.
    const CORRECTED_ENTRY: &str = "[Desktop Entry]\nType=Application\nVersion=1.0\n\
        Name=Redline\nComment=com.fabiosm46.redline startup script\n\
        Exec=/usr/bin/redline --autostart\nStartupNotify=false\nTerminal=false";

    /// A launcher that writes in a temporary folder what the plugin writes in the real one.
    struct FileLauncher(std::path::PathBuf);

    impl Launcher for FileLauncher {
        fn enable(&self) -> Result<(), String> {
            fs::write(&self.0, PLUGIN_ENTRY).map_err(|error| error.to_string())
        }

        fn disable(&self) -> Result<(), String> {
            fs::remove_file(&self.0).map_err(|error| error.to_string())
        }

        fn is_enabled(&self) -> Result<bool, String> {
            Ok(self.0.exists())
        }
    }

    impl Folder {
        fn entry(&self) -> std::path::PathBuf {
            self.0.join(format!("{ENTRY_NAME}.desktop"))
        }

        fn launcher(&self) -> FileLauncher {
            FileLauncher(self.entry())
        }

        /// Makes the rewrite fail, for any user: a folder sits where the temporary file goes.
        fn block_rewrite(&self) {
            let name = format!("{ENTRY_NAME}.desktop.tmp.{}", std::process::id());
            fs::create_dir(self.0.join(name)).unwrap();
        }

        fn files(&self) -> Vec<String> {
            let mut files: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|file| file.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            files.sort();
            files
        }
    }

    #[test]
    fn turning_on_writes_the_entry_under_the_identifier_with_the_display_name() {
        let folder = Folder::new("enabled");
        let launcher = folder.launcher();

        assert_eq!(write_and_name(&launcher, Some(&folder.0), true), Ok(true));
        assert_eq!(fs::read_to_string(folder.entry()).unwrap(), CORRECTED_ENTRY);
        assert_eq!(folder.files(), [format!("{ENTRY_NAME}.desktop")]);
    }

    #[test]
    fn turning_off_removes_the_entry_and_corrects_nothing() {
        let folder = Folder::new("disabled");
        let launcher = folder.launcher();
        launcher.enable().unwrap();

        assert_eq!(write_and_name(&launcher, Some(&folder.0), false), Ok(false));
        assert!(folder.files().is_empty());
    }

    #[test]
    fn a_failed_correction_keeps_the_entry_and_the_state_read_from_the_system() {
        let folder = Folder::new("enabled-blocked");
        folder.block_rewrite();
        let launcher = folder.launcher();

        assert_eq!(write_and_name(&launcher, Some(&folder.0), true), Ok(true));
        assert_eq!(fs::read_to_string(folder.entry()).unwrap(), PLUGIN_ENTRY);
    }

    #[test]
    fn the_entry_that_replaces_an_old_one_shows_the_display_name() {
        let folder = Folder::new("repair-legacy");
        fs::write(folder.legacy(), OLD_ENTRY).unwrap();

        repair(&folder.0, &folder.launcher());

        assert_eq!(fs::read_to_string(folder.entry()).unwrap(), CORRECTED_ENTRY);
        assert_eq!(folder.files(), [format!("{ENTRY_NAME}.desktop")]);
    }

    #[test]
    fn an_existing_entry_is_corrected_at_startup() {
        let folder = Folder::new("repair-existing");
        fs::write(folder.entry(), PLUGIN_ENTRY).unwrap();

        repair(&folder.0, &folder.launcher());

        assert_eq!(fs::read_to_string(folder.entry()).unwrap(), CORRECTED_ENTRY);
        assert_eq!(folder.files(), [format!("{ENTRY_NAME}.desktop")]);
    }

    #[test]
    fn an_entry_already_corrected_is_not_rewritten() {
        let folder = Folder::new("corrected");
        fs::write(folder.entry(), CORRECTED_ENTRY).unwrap();
        // A rewrite would fail here.
        folder.block_rewrite();

        assert!(correct_name(&folder.entry()).is_ok());
    }

    #[test]
    fn no_entry_is_not_a_failure_and_creates_nothing() {
        let folder = Folder::new("no-entry");

        assert!(correct_name(&folder.entry()).is_ok());
        assert!(folder.files().is_empty());
    }

    #[test]
    fn an_entry_turned_off_in_the_desktop_gets_the_name_and_stays_off() {
        for key in ["Hidden=true", "X-GNOME-Autostart-enabled=false"] {
            let folder = Folder::new(&format!("off-{key}"));
            fs::write(folder.entry(), format!("{PLUGIN_ENTRY}\n{key}\n")).unwrap();

            assert!(correct_name(&folder.entry()).is_ok(), "{key}");
            assert_eq!(
                fs::read_to_string(folder.entry()).unwrap(),
                format!("{CORRECTED_ENTRY}\n{key}\n")
            );
        }
    }

    #[test]
    fn a_link_at_the_temporary_path_is_not_written_through() {
        let folder = Folder::new("link");
        fs::write(folder.entry(), PLUGIN_ENTRY).unwrap();
        let target = folder.0.join("target");
        fs::write(&target, "untouched").unwrap();
        let tmp = format!("{ENTRY_NAME}.desktop.tmp.{}", std::process::id());
        std::os::unix::fs::symlink(&target, folder.0.join(tmp)).unwrap();

        assert!(correct_name(&folder.entry()).is_ok());
        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
        assert_eq!(fs::read_to_string(folder.entry()).unwrap(), CORRECTED_ENTRY);
    }

    #[test]
    fn an_entry_that_is_not_redlines_is_not_touched() {
        for content in [
            // No `--autostart`: not what the plugin writes.
            "[Desktop Entry]\nName=com.fabiosm46.redline\nExec=/usr/bin/redline\n",
            // Redline's launch under a name the user chose.
            "[Desktop Entry]\nName=My GPU panel\nExec=/usr/bin/redline --autostart\n",
            // Not the whole line.
            "[Desktop Entry]\nName=com.fabiosm46.redline2\nExec=/usr/bin/redline --autostart\n",
        ] {
            let folder = Folder::new("foreign");
            fs::write(folder.entry(), content).unwrap();

            assert!(correct_name(&folder.entry()).is_ok());
            assert_eq!(fs::read_to_string(folder.entry()).unwrap(), content);
        }
    }

    #[test]
    fn a_failed_rewrite_is_reported_and_leaves_the_working_entry() {
        let folder = Folder::new("blocked");
        fs::write(folder.entry(), PLUGIN_ENTRY).unwrap();
        folder.block_rewrite();

        assert!(correct_name(&folder.entry()).is_err());
        assert_eq!(fs::read_to_string(folder.entry()).unwrap(), PLUGIN_ENTRY);
    }

    #[test]
    fn only_the_name_line_changes_and_the_line_endings_stay() {
        assert_eq!(
            with_display_name("Name=com.fabiosm46.redline\nExec=a\n").as_deref(),
            Some("Name=Redline\nExec=a\n")
        );
        assert_eq!(
            with_display_name("Exec=a\nName=com.fabiosm46.redline").as_deref(),
            Some("Exec=a\nName=Redline")
        );
        assert_eq!(with_display_name("Name=Redline\n"), None);
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
