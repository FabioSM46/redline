//! The in-game overlay: MangoHud, configured by Redline through its user config file (D12).
//!
//! - **Detection** ([`mangohud_installed`], the one place that answers "is MangoHud here?"): its
//!   implicit Vulkan layer manifest or the `mangohud` launcher on PATH. Nothing is run: MangoHud
//!   has no version flag, and missing it is a disabled feature with [`MISSING`], not an error.
//! - **The file**: [`render`] turns the overlay settings (stored by `settings`) into
//!   `<config dir>/MangoHud/MangoHud.conf`, written atomically. Its first line is [`MARKER`].
//! - **The user's own file**: a `MangoHud.conf` without the marker is never overwritten silently.
//!   `set_overlay` saves the settings and leaves the file alone; `take_over_overlay_config`, which
//!   the frontend calls only after asking, keeps it as [`BACKUP_FILE`] and writes Redline's.
//!
//! Writing the file changes nothing until a game runs with MangoHud, which only `MANGOHUD=1` in
//! the game's environment does: hence `launch_hint`. Redline sets that variable nowhere.
//!
//! The keys are in both MangoHud 0.6.9.1 and 0.8.4 (see `docs/architecture.md`).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use redline_core::thumbnail::find_program;
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::settings::{
    self, OverlayMetric as Metric, OverlayPosition, OverlayPreset, OverlaySettings, SettingsState,
};

/// What to do when MangoHud is missing.
pub const MISSING: &str = "MangoHud is not installed. Install the mangohud package from your \
    distribution's packages to use the in-game overlay.";
/// The Steam launch option that turns MangoHud on for one game.
pub const LAUNCH_HINT: &str = "mangohud %command%";
/// The first line of a file Redline owns.
pub const MARKER: &str = "# Managed by Redline";
/// The user's own file, once Redline has taken over.
pub const BACKUP_FILE: &str = "MangoHud.conf.redline-backup";
const NOTICE: &str = "# Changed from Redline: edits made here are replaced at the next save.";
const CONFIG_FOLDER: &str = "MangoHud";
const CONFIG_FILE: &str = "MangoHud.conf";
const LAUNCHER: &str = "mangohud";
const LAYER_FOLDER: &str = "vulkan/implicit_layer.d";
/// The value of `XDG_DATA_DIRS` when it is unset or empty, as the XDG specification says.
const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";
const BACKUP_EXISTS: &str = "Your MangoHud.conf was not changed: a MangoHud.conf.redline-backup \
    already exists next to it. Move that file away, then try again.";
const NO_CONFIG_FOLDER: &str = "Could not write the MangoHud configuration: no config folder.";
const UNAVAILABLE: &str = "The overlay is not available: restart the app.";

/// The figures of each preset. `off` starts hidden; its key shows [`MINIMAL`].
const MINIMAL: &[Metric] = &[Metric::Fps, Metric::Frametime];
const STANDARD: &[Metric] = &[
    Metric::Fps,
    Metric::Frametime,
    Metric::GpuTemp,
    Metric::GpuPower,
    Metric::GpuCoreClock,
    Metric::CpuTemp,
];
const FULL: &[Metric] = &[
    Metric::Fps,
    Metric::Frametime,
    Metric::GpuTemp,
    Metric::GpuPower,
    Metric::GpuCoreClock,
    Metric::Vram,
    Metric::CpuTemp,
    Metric::Ram,
];

/// Who owns `MangoHud.conf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigState {
    Absent,
    Managed,
    UserConfig,
}

/// The overlay, as `overlay_status` returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverlayStatus {
    pub available: bool,
    /// What to install; `null` when `available`.
    pub unavailable_reason: Option<String>,
    pub config: ConfigState,
    pub settings: OverlaySettings,
    pub launch_hint: &'static str,
}

/// The managed state.
pub struct Overlay {
    /// `None` when Tauri could not resolve the config folder: nothing can be written.
    file: Option<PathBuf>,
    /// Held from reading who owns the file until it is written, by one command at a time.
    busy: Mutex<()>,
}

/// Registers the managed state. Writes nothing: the file appears on the first `set_overlay`.
pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let file = match app.path().config_dir() {
        Ok(dir) => Some(config_file(&dir)),
        Err(error) => {
            eprintln!("redline: config folder not available for the overlay: {error}");
            None
        }
    };
    app.manage(Overlay {
        file,
        busy: Mutex::new(()),
    });
}

/// `MangoHud.conf` inside `config_dir` (`$XDG_CONFIG_HOME`, otherwise `~/.config`).
fn config_file(config_dir: &Path) -> PathBuf {
    config_dir.join(CONFIG_FOLDER).join(CONFIG_FILE)
}

/// Whether MangoHud is on this system, for this process's PATH and `XDG_DATA_DIRS`. Reads only the
/// file system, so it can be called anywhere.
pub fn mangohud_present() -> bool {
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    let data_dirs: Option<OsString> = std::env::var_os("XDG_DATA_DIRS");
    mangohud_installed(&search_path, &layer_dirs(data_dirs.as_deref()))
}

/// The folders that can hold MangoHud's layer manifest, from the value of `XDG_DATA_DIRS`.
pub fn layer_dirs(xdg_data_dirs: Option<&OsStr>) -> Vec<PathBuf> {
    let listed = xdg_data_dirs
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or(OsStr::new(DEFAULT_DATA_DIRS));
    let mut dirs: Vec<PathBuf> = Vec::new();
    let roots = std::env::split_paths(listed)
        // The XDG specification ignores relative entries.
        .filter(|dir| dir.is_absolute())
        .chain(["/usr/share", "/etc"].map(PathBuf::from));
    for root in roots {
        let dir = root.join(LAYER_FOLDER);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Whether one of `layer_dirs` holds a `MangoHud*.json` manifest or `mangohud` is on
/// `search_path`.
pub fn mangohud_installed(search_path: &OsStr, layer_dirs: &[PathBuf]) -> bool {
    layer_dirs.iter().any(|dir| has_manifest(dir)) || find_program(LAUNCHER, search_path).is_some()
}

/// A missing or unreadable folder holds no manifest.
fn has_manifest(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with("MangoHud")
            && name.ends_with(".json")
            && fs::metadata(entry.path()).is_ok_and(|meta| meta.is_file())
    })
}

fn position_value(position: OverlayPosition) -> &'static str {
    match position {
        OverlayPosition::TopLeft => "top-left",
        OverlayPosition::TopRight => "top-right",
        OverlayPosition::BottomLeft => "bottom-left",
        OverlayPosition::BottomRight => "bottom-right",
    }
}

/// The figures `settings` asks for: the advanced list when there is one, otherwise the preset's.
fn chosen_metrics(settings: &OverlaySettings) -> &[Metric] {
    if !settings.metrics.is_empty() {
        return &settings.metrics;
    }
    match settings.preset {
        OverlayPreset::Off | OverlayPreset::Minimal => MINIMAL,
        OverlayPreset::Standard => STANDARD,
        OverlayPreset::Full => FULL,
    }
}

/// The text of `MangoHud.conf`. Pure.
///
/// `legacy_layout=0` is what makes the file mean what it lists: otherwise MangoHud also draws its
/// defaults (GPU, CPU, FPS and the frame graph) whatever is written. With it, MangoHud draws one
/// element per line, in file order, and some keys are not elements but details of one:
/// `gpu_temp`, `gpu_power` and `gpu_core_clock` appear only inside `gpu_stats`, `cpu_temp` inside
/// `cpu_stats` and `frametime` inside `fps`. So the parent line is written with them, and
/// `frametime` (on by default) is turned off explicitly. The order is fixed, whatever the order
/// of the list, and a figure listed twice is written once.
pub fn render(settings: &OverlaySettings) -> String {
    let chosen = chosen_metrics(settings);
    let has = |metric: Metric| chosen.contains(&metric);
    let mut lines = vec![
        MARKER.to_owned(),
        NOTICE.to_owned(),
        "legacy_layout=0".to_owned(),
        format!("position={}", position_value(settings.position)),
        format!("toggle_hud={}", settings.toggle_key),
    ];
    let mut add = |line: &str| lines.push(line.to_owned());
    if settings.preset == OverlayPreset::Off {
        add("no_display");
    }
    let gpu_details = [
        (Metric::GpuTemp, "gpu_temp"),
        (Metric::GpuPower, "gpu_power"),
        (Metric::GpuCoreClock, "gpu_core_clock"),
    ];
    if gpu_details.iter().any(|(metric, _)| has(*metric)) {
        add("gpu_stats");
    }
    for (metric, key) in gpu_details {
        if has(metric) {
            add(key);
        }
    }
    if has(Metric::CpuTemp) {
        add("cpu_stats");
        add("cpu_temp");
    }
    if has(Metric::Vram) {
        add("vram");
    }
    if has(Metric::Ram) {
        add("ram");
    }
    if has(Metric::Fps) || has(Metric::Frametime) {
        add("fps");
        add(if has(Metric::Frametime) {
            "frametime"
        } else {
            "frametime=0"
        });
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Who owns `file`. Anything Redline cannot show to be its own is the user's: a dangling link and
/// an unreadable file exist, and writing over them would lose them.
fn config_state(file: &Path) -> ConfigState {
    match fs::symlink_metadata(file) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return ConfigState::Absent,
        Err(_) => return ConfigState::UserConfig,
        Ok(_) => {}
    }
    // Only a regular file is opened: a FIFO would block.
    let regular = fs::metadata(file).is_ok_and(|meta| meta.is_file());
    let first_line_is_marker = regular
        && fs::read(file).is_ok_and(|text| {
            let first = text.split(|byte| *byte == b'\n').next().unwrap_or_default();
            first.trim_ascii() == MARKER.as_bytes()
        });
    if first_line_is_marker {
        ConfigState::Managed
    } else {
        ConfigState::UserConfig
    }
}

/// Writes `text` to a temporary file next to `file` and returns its path; the caller renames it.
fn write_temp(file: &Path, text: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(file.parent().unwrap_or_else(|| Path::new(".")))?;
    let tmp = file.with_extension(format!("conf.tmp.{}", std::process::id()));
    let written = (|| {
        let mut out = fs::File::create(&tmp)?;
        out.write_all(text.as_bytes())?;
        out.sync_all()
    })();
    match written {
        Ok(()) => Ok(tmp),
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            Err(error)
        }
    }
}

/// Replaces `file` with the rendered settings, atomically.
fn write_config(file: &Path, settings: &OverlaySettings) -> io::Result<()> {
    let tmp = write_temp(file, &render(settings))?;
    let renamed = fs::rename(&tmp, file);
    if renamed.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    renamed
}

fn write_failed(error: io::Error) -> String {
    format!(
        "Could not write the MangoHud configuration ({}).",
        error.kind()
    )
}

/// Writes the file unless it is the user's own, which stays as it is.
fn write_unless_user_config(file: &Path, settings: &OverlaySettings) -> Result<(), String> {
    match config_state(file) {
        ConfigState::UserConfig => Ok(()),
        ConfigState::Absent | ConfigState::Managed => {
            write_config(file, settings).map_err(write_failed)
        }
    }
}

/// Keeps the user's file as [`BACKUP_FILE`] and writes Redline's. An existing backup is never
/// replaced: nothing changes and the answer says so.
///
/// The new file is ready before anything moves, and a hard link is the move: it fails if the
/// backup exists, leaves a symbolic link a symbolic link, and `MangoHud.conf` never goes missing
/// on the way.
fn take_over(file: &Path, settings: &OverlaySettings) -> Result<(), String> {
    if config_state(file) != ConfigState::UserConfig {
        return write_config(file, settings).map_err(write_failed);
    }
    let backup = file.with_file_name(BACKUP_FILE);
    let tmp = write_temp(file, &render(settings)).map_err(write_failed)?;
    if let Err(error) = fs::hard_link(file, &backup) {
        let _ = fs::remove_file(&tmp);
        return Err(if error.kind() == io::ErrorKind::AlreadyExists {
            BACKUP_EXISTS.to_owned()
        } else {
            write_failed(error)
        });
    }
    if let Err(error) = fs::rename(&tmp, file) {
        let _ = fs::remove_file(&tmp);
        let _ = fs::remove_file(&backup);
        return Err(write_failed(error));
    }
    Ok(())
}

fn status_of(available: bool, config: ConfigState, settings: OverlaySettings) -> OverlayStatus {
    OverlayStatus {
        available,
        unavailable_reason: (!available).then(|| MISSING.to_owned()),
        config,
        settings,
        launch_hint: LAUNCH_HINT,
    }
}

impl Overlay {
    fn status(&self, settings: OverlaySettings) -> OverlayStatus {
        let config = self
            .file
            .as_deref()
            .map_or(ConfigState::Absent, config_state);
        status_of(mangohud_present(), config, settings)
    }

    /// The file to write, once the feature is known to be available.
    fn writable_file(&self) -> Result<&Path, String> {
        if !mangohud_present() {
            return Err(MISSING.to_owned());
        }
        self.file
            .as_deref()
            .ok_or_else(|| NO_CONFIG_FOLDER.to_owned())
    }
}

/// Whether MangoHud is here, who owns its config file, and the saved settings. Changes nothing.
#[tauri::command]
pub fn overlay_status(
    overlay: State<'_, Overlay>,
    settings: State<'_, SettingsState>,
) -> OverlayStatus {
    let _busy = overlay.busy.lock();
    overlay.status(settings.current().overlay)
}

/// Saves the settings and writes them to `MangoHud.conf`. A file that is the user's own stays as
/// it is: the status says `user_config`, and the settings reach it on a take-over.
#[tauri::command]
pub fn set_overlay(
    overlay: State<'_, Overlay>,
    state: State<'_, SettingsState>,
    settings: OverlaySettings,
) -> Result<OverlayStatus, String> {
    let _busy = overlay.busy.lock().map_err(|_| UNAVAILABLE.to_owned())?;
    let file = overlay.writable_file()?;
    let saved = settings::save_overlay(&state, settings)?;
    write_unless_user_config(file, &saved)?;
    Ok(overlay.status(saved))
}

/// Keeps the user's `MangoHud.conf` as `MangoHud.conf.redline-backup` and writes Redline's. The
/// frontend calls this only after the user has agreed.
#[tauri::command]
pub fn take_over_overlay_config(
    overlay: State<'_, Overlay>,
    state: State<'_, SettingsState>,
) -> Result<OverlayStatus, String> {
    let _busy = overlay.busy.lock().map_err(|_| UNAVAILABLE.to_owned())?;
    let file = overlay.writable_file()?;
    let settings = state.current().overlay;
    take_over(file, &settings)?;
    Ok(overlay.status(settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn temp_dir(name: &str) -> TempDir {
        TempDir::new("overlay", name)
    }

    fn settings(preset: OverlayPreset) -> OverlaySettings {
        OverlaySettings {
            preset,
            ..OverlaySettings::default()
        }
    }

    /// The lines after the marker and the notice.
    fn body(settings: &OverlaySettings) -> Vec<String> {
        render(settings)
            .lines()
            .skip(2)
            .map(str::to_owned)
            .collect()
    }

    /// The lines after the options every file has.
    fn metric_lines(settings: &OverlaySettings) -> Vec<String> {
        body(settings).split_off(3)
    }

    /// A temporary `XDG_CONFIG_HOME` and the config file inside it, which does not exist yet.
    fn config_home(name: &str) -> (TempDir, PathBuf) {
        let dir = temp_dir(name);
        let file = config_file(&dir);
        (dir, file)
    }

    fn user_file(file: &Path, text: &str) {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }

    fn files_next_to(file: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_first_line_is_the_marker_and_the_file_ends_with_a_new_line() {
        let text = render(&OverlaySettings::default());
        assert_eq!(text.lines().next(), Some("# Managed by Redline"));
        assert!(text.ends_with("frametime\n"));
    }

    #[test]
    fn the_options_of_every_file_come_first() {
        assert_eq!(
            body(&OverlaySettings::default())[..3],
            [
                "legacy_layout=0",
                "position=top-left",
                "toggle_hud=Shift_R+F12"
            ]
        );
    }

    #[test]
    fn every_preset_writes_its_figures() {
        assert_eq!(
            metric_lines(&settings(OverlayPreset::Minimal)),
            ["fps", "frametime"]
        );
        assert_eq!(
            metric_lines(&settings(OverlayPreset::Standard)),
            [
                "gpu_stats",
                "gpu_temp",
                "gpu_power",
                "gpu_core_clock",
                "cpu_stats",
                "cpu_temp",
                "fps",
                "frametime"
            ]
        );
        assert_eq!(
            metric_lines(&settings(OverlayPreset::Full)),
            [
                "gpu_stats",
                "gpu_temp",
                "gpu_power",
                "gpu_core_clock",
                "cpu_stats",
                "cpu_temp",
                "vram",
                "ram",
                "fps",
                "frametime"
            ]
        );
    }

    #[test]
    fn off_starts_hidden_and_its_key_shows_the_minimal_figures() {
        assert_eq!(
            metric_lines(&settings(OverlayPreset::Off)),
            ["no_display", "fps", "frametime"]
        );
        for preset in [
            OverlayPreset::Minimal,
            OverlayPreset::Standard,
            OverlayPreset::Full,
        ] {
            assert!(!render(&settings(preset)).contains("no_display"));
        }
    }

    #[test]
    fn every_position_is_a_mangohud_value() {
        for (position, value) in [
            (OverlayPosition::TopLeft, "top-left"),
            (OverlayPosition::TopRight, "top-right"),
            (OverlayPosition::BottomLeft, "bottom-left"),
            (OverlayPosition::BottomRight, "bottom-right"),
        ] {
            let settings = OverlaySettings {
                position,
                ..OverlaySettings::default()
            };
            assert!(body(&settings).contains(&format!("position={value}")));
            // The same spelling the frontend sends.
            assert_eq!(serde_json::to_value(position).unwrap(), json!(value));
        }
    }

    #[test]
    fn the_toggle_key_is_written_as_it_is() {
        let settings = OverlaySettings {
            toggle_key: "Control_L+F9".into(),
            ..OverlaySettings::default()
        };
        assert_eq!(body(&settings)[2], "toggle_hud=Control_L+F9");
    }

    #[test]
    fn the_advanced_list_replaces_the_figures_of_the_preset() {
        let settings = OverlaySettings {
            preset: OverlayPreset::Full,
            metrics: vec![Metric::Ram, Metric::Fps],
            ..OverlaySettings::default()
        };
        assert_eq!(metric_lines(&settings), ["ram", "fps", "frametime=0"]);
    }

    #[test]
    fn the_advanced_list_with_off_still_starts_hidden() {
        let settings = OverlaySettings {
            preset: OverlayPreset::Off,
            metrics: vec![Metric::Vram],
            ..OverlaySettings::default()
        };
        assert_eq!(metric_lines(&settings), ["no_display", "vram"]);
    }

    #[test]
    fn a_detail_brings_the_line_it_is_drawn_in() {
        let with = |metric| OverlaySettings {
            metrics: vec![metric],
            ..OverlaySettings::default()
        };
        assert_eq!(
            metric_lines(&with(Metric::GpuPower)),
            ["gpu_stats", "gpu_power"]
        );
        assert_eq!(
            metric_lines(&with(Metric::CpuTemp)),
            ["cpu_stats", "cpu_temp"]
        );
        assert_eq!(metric_lines(&with(Metric::Frametime)), ["fps", "frametime"]);
    }

    #[test]
    fn the_order_of_the_list_and_duplicates_do_not_change_the_file() {
        let list = |metrics: Vec<Metric>| OverlaySettings {
            metrics,
            ..OverlaySettings::default()
        };
        assert_eq!(
            render(&list(vec![Metric::Fps, Metric::Vram, Metric::Fps])),
            render(&list(vec![Metric::Vram, Metric::Fps]))
        );
    }

    #[test]
    fn only_keys_known_to_both_verified_versions_are_written() {
        const KNOWN: &[&str] = &[
            "legacy_layout",
            "position",
            "toggle_hud",
            "no_display",
            "gpu_stats",
            "gpu_temp",
            "gpu_power",
            "gpu_core_clock",
            "cpu_stats",
            "cpu_temp",
            "vram",
            "ram",
            "fps",
            "frametime",
        ];
        for preset in [OverlayPreset::Off, OverlayPreset::Full] {
            for line in body(&settings(preset)) {
                let key = line.split('=').next().unwrap();
                assert!(KNOWN.contains(&key), "{line}");
            }
        }
    }

    #[test]
    fn a_missing_file_is_absent() {
        let (_dir, file) = config_home("absent");
        assert_eq!(config_state(&file), ConfigState::Absent);
    }

    #[test]
    fn the_marker_on_the_first_line_makes_the_file_redlines() {
        let (_dir, file) = config_home("managed");
        write_config(&file, &OverlaySettings::default()).unwrap();
        assert_eq!(config_state(&file), ConfigState::Managed);
        user_file(&file, "# Managed by Redline\r\nfps\r\n");
        assert_eq!(config_state(&file), ConfigState::Managed);
        user_file(&file, "# Managed by Redline");
        assert_eq!(config_state(&file), ConfigState::Managed);
    }

    #[test]
    fn anything_else_is_the_users() {
        let (_dir, file) = config_home("user");
        for text in [
            "",
            "fps\n",
            "\n# Managed by Redline\n",
            "fps\n# Managed by Redline\n",
            "# Managed by Redline, not really\n",
        ] {
            user_file(&file, text);
            assert_eq!(config_state(&file), ConfigState::UserConfig, "{text:?}");
        }
        fs::write(&file, [0xff, 0xfe, b'\n']).unwrap();
        assert_eq!(config_state(&file), ConfigState::UserConfig);
    }

    #[test]
    fn what_cannot_be_read_is_never_taken_for_absent() {
        let (dir, file) = config_home("not-a-file");
        // A folder where the file should be.
        fs::create_dir_all(&file).unwrap();
        assert_eq!(config_state(&file), ConfigState::UserConfig);
        fs::remove_dir(&file).unwrap();
        // A link to a file that is gone: `exists()` would call this absent.
        symlink(dir.join("gone"), &file).unwrap();
        assert_eq!(config_state(&file), ConfigState::UserConfig);
    }

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temporary_file() {
        let (_dir, file) = config_home("write");
        for preset in [OverlayPreset::Full, OverlayPreset::Minimal] {
            write_config(&file, &settings(preset)).unwrap();
            assert_eq!(
                fs::read_to_string(&file).unwrap(),
                render(&settings(preset))
            );
        }
        assert_eq!(files_next_to(&file), [CONFIG_FILE]);
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file() {
        let (_dir, file) = config_home("write-failed");
        write_config(&file, &OverlaySettings::default()).unwrap();
        // A folder that is not empty at the target: `rename` over it fails.
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        fs::write(file.join("inside"), "").unwrap();
        assert!(write_config(&file, &OverlaySettings::default()).is_err());
        assert_eq!(files_next_to(&file), [CONFIG_FILE]);
    }

    #[test]
    fn saving_writes_an_absent_or_managed_file_and_never_the_users() {
        let (_dir, file) = config_home("save");
        let full = settings(OverlayPreset::Full);
        write_unless_user_config(&file, &full).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), render(&full));

        let minimal = settings(OverlayPreset::Minimal);
        write_unless_user_config(&file, &minimal).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), render(&minimal));

        user_file(&file, "fps\n");
        write_unless_user_config(&file, &full).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "fps\n");
        assert_eq!(files_next_to(&file), [CONFIG_FILE]);
    }

    #[test]
    fn taking_over_keeps_the_users_file_as_the_backup() {
        let (_dir, file) = config_home("take-over");
        user_file(&file, "fps\ngpu_stats\n");
        let wanted = settings(OverlayPreset::Full);
        take_over(&file, &wanted).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), render(&wanted));
        assert_eq!(config_state(&file), ConfigState::Managed);
        assert_eq!(
            fs::read_to_string(file.with_file_name(BACKUP_FILE)).unwrap(),
            "fps\ngpu_stats\n"
        );
        assert_eq!(files_next_to(&file), [CONFIG_FILE, BACKUP_FILE]);
    }

    #[test]
    fn an_existing_backup_is_never_overwritten() {
        let (_dir, file) = config_home("take-over-backup");
        user_file(&file, "mine\n");
        let backup = file.with_file_name(BACKUP_FILE);
        fs::write(&backup, "older\n").unwrap();
        assert_eq!(
            take_over(&file, &OverlaySettings::default()),
            Err(BACKUP_EXISTS.to_owned())
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "mine\n");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "older\n");
        assert_eq!(files_next_to(&file), [CONFIG_FILE, BACKUP_FILE]);
    }

    #[test]
    fn taking_over_a_managed_or_absent_file_makes_no_backup() {
        let (_dir, file) = config_home("take-over-nothing");
        take_over(&file, &OverlaySettings::default()).unwrap();
        take_over(&file, &settings(OverlayPreset::Full)).unwrap();
        assert_eq!(config_state(&file), ConfigState::Managed);
        assert_eq!(files_next_to(&file), [CONFIG_FILE]);
    }

    #[test]
    fn a_link_to_the_users_dotfile_is_kept_as_a_link() {
        let (dir, file) = config_home("take-over-link");
        let dotfile = dir.join("dotfiles-mangohud.conf");
        fs::write(&dotfile, "mine\n").unwrap();
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        symlink(&dotfile, &file).unwrap();
        assert_eq!(config_state(&file), ConfigState::UserConfig);
        take_over(&file, &OverlaySettings::default()).unwrap();
        let backup = file.with_file_name(BACKUP_FILE);
        assert!(fs::symlink_metadata(&backup).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&dotfile).unwrap(), "mine\n");
        assert_eq!(config_state(&file), ConfigState::Managed);
    }

    #[test]
    fn what_cannot_be_moved_stops_the_take_over_and_cleans_up() {
        let (_dir, file) = config_home("take-over-folder");
        fs::create_dir_all(&file).unwrap();
        // A folder cannot be hard linked.
        let error = take_over(&file, &OverlaySettings::default()).unwrap_err();
        assert!(error.contains("Could not write"), "{error}");
        assert!(file.is_dir());
        assert_eq!(files_next_to(&file), [CONFIG_FILE]);
    }

    /// A fake data dir with `name` in its layer folder; returns that folder.
    fn fake_manifest(dir: &Path, name: &str) -> PathBuf {
        let layers = dir.join("data").join(LAYER_FOLDER);
        fs::create_dir_all(&layers).unwrap();
        fs::write(layers.join(name), "{}").unwrap();
        layers
    }

    #[test]
    fn the_layer_manifest_is_enough() {
        let dir = temp_dir("manifest");
        for name in ["MangoHud.x86_64.json", "MangoHud.json", "MangoHud_32.json"] {
            let layers = fake_manifest(&dir, name);
            assert!(
                mangohud_installed(OsStr::new(""), std::slice::from_ref(&layers)),
                "{name}"
            );
            fs::remove_file(layers.join(name)).unwrap();
        }
    }

    #[test]
    fn the_launcher_on_path_is_enough() {
        let dir = temp_dir("launcher");
        let launcher = dir.join(LAUNCHER);
        fs::write(&launcher, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(mangohud_installed(dir.as_os_str(), &[]));
        // A file that cannot be run is no launcher.
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!mangohud_installed(dir.as_os_str(), &[]));
    }

    #[test]
    fn nothing_found_is_not_installed() {
        let dir = temp_dir("nothing");
        let other = fake_manifest(&dir, "VkLayer_MESA_device_select.json");
        fake_manifest(&dir, "MangoHud.txt");
        fake_manifest(&dir, "NotMangoHud.json");
        assert!(!mangohud_installed(
            dir.as_os_str(),
            &[other, dir.join("missing")]
        ));
    }

    #[test]
    fn a_folder_named_like_a_manifest_is_not_one() {
        let dir = temp_dir("manifest-folder");
        let layers = fake_manifest(&dir, "other.json");
        fs::create_dir(layers.join("MangoHud.json")).unwrap();
        assert!(!mangohud_installed(OsStr::new(""), &[layers]));
    }

    #[test]
    fn the_layer_folders_follow_xdg_data_dirs_then_the_system() {
        let dirs = layer_dirs(Some(OsStr::new("/data/one:relative/two::/usr/share/")));
        assert_eq!(
            dirs,
            [
                "/data/one/vulkan/implicit_layer.d",
                "/usr/share/vulkan/implicit_layer.d",
                "/etc/vulkan/implicit_layer.d",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn without_xdg_data_dirs_the_layer_folders_are_the_default_ones() {
        let wanted = [
            "/usr/local/share/vulkan/implicit_layer.d",
            "/usr/share/vulkan/implicit_layer.d",
            "/etc/vulkan/implicit_layer.d",
        ]
        .map(PathBuf::from);
        assert_eq!(layer_dirs(None), wanted);
        assert_eq!(layer_dirs(Some(OsStr::new(""))), wanted);
    }

    #[test]
    fn the_config_file_is_inside_the_config_folder() {
        assert_eq!(
            config_file(Path::new("/config")),
            Path::new("/config/MangoHud/MangoHud.conf")
        );
    }

    #[test]
    fn a_missing_tool_is_a_sentence_not_an_error() {
        let status = status_of(false, ConfigState::Absent, OverlaySettings::default());
        assert_eq!(status.unavailable_reason.as_deref(), Some(MISSING));
        assert!(MISSING.contains("mangohud"));
        let status = status_of(true, ConfigState::Absent, OverlaySettings::default());
        assert_eq!(status.unavailable_reason, None);
    }

    #[test]
    fn serializes_the_contract() {
        let status = status_of(true, ConfigState::UserConfig, OverlaySettings::default());
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({
                "available": true,
                "unavailable_reason": null,
                "config": "user_config",
                "settings": {
                    "preset": "standard",
                    "position": "top-left",
                    "metrics": [],
                    "toggle_key": "Shift_R+F12",
                },
                "launch_hint": "mangohud %command%",
            })
        );
        for (state, value) in [
            (ConfigState::Absent, "absent"),
            (ConfigState::Managed, "managed"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), json!(value));
        }
    }
}
