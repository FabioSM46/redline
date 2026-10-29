//! Persistent settings (D8: Settings → General, Capture, and the replay options of Record).
//!
//! The settings live in `settings.json` inside the folder Tauri resolves with
//! `app.path().app_config_dir()`. A missing file means the defaults. An unreadable or invalid
//! file also means the defaults, with the reason on stderr: a damaged file never blocks the app.
//! A single bad field (an unknown theme, a frame rate out of range) falls back to its own default
//! and keeps the others; that holds inside `recording` too. Writes are atomic: a temporary file in
//! the same folder, then `rename`.
//!
//! `recording` holds how replays and recordings are made. Its allowed values are GPU Screen
//! Recorder's ([`recorder::CODECS`], [`recorder::QUALITIES`], [`recorder::REPLAY_SECONDS`],
//! [`recorder::FPS`]), kept in one place in `redline-core`. The folders default to
//! `<video folder>/Redline/{Replays,Recordings}`; a folder that does not exist is created when a
//! replay starts.
//!
//! The portal session token (`-portal-session-token-filepath`) is a file owned by Redline in the
//! app data folder ([`PORTAL_TOKEN_FILE`]). Saving a different `capture` deletes it, so the
//! desktop asks for the screen again (D8).
//!
//! Other modules read the settings through [`recording_settings`] (for example the replay and
//! recording folders), never from the file.
//!
//! Starting with the system is not a setting stored here: the operating system owns it (see
//! `autostart`).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use redline_core::recorder::{self, AudioTrack, ReplayConfig};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Manager, Runtime, State};

/// The name of the settings file inside the app's config folder.
pub const SETTINGS_FILE: &str = "settings.json";
/// The portal session token, inside the app's data folder.
pub const PORTAL_TOKEN_FILE: &str = "portal-session-token";
const VIDEO_SUBFOLDER: &str = "Redline";
const REPLAY_FOLDER: &str = "Replays";
const RECORDING_FOLDER: &str = "Recordings";
const DEFAULT_REPLAY_SECONDS: u32 = 60;
const DEFAULT_FPS: u32 = 60;
/// Capture values gsr accepts for `-w` that are neither the portal nor a monitor.
const NOT_A_MONITOR: &[&str] = &["focused", "region", "screen-direct", "window"];
const UNAVAILABLE: &str = "Settings are not available: restart the app.";

/// The interface theme: `system` follows the desktop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// How replays and recordings are made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingSettings {
    /// The replay length in seconds.
    pub replay_seconds: u32,
    /// One entry per audio track of the file; the sources of a track are mixed. A source is
    /// `default_output`, `default_input`, `device:<name>`, `app:<name>` or `app-inverse:<name>`.
    pub audio_tracks: Vec<Vec<String>>,
    pub replay_dir: PathBuf,
    pub recording_dir: PathBuf,
    /// `portal`, or a monitor name such as `DP-2`.
    pub capture: String,
    /// `None`: GPU Screen Recorder chooses.
    pub codec: Option<String>,
    pub quality: String,
    pub fps: u32,
    /// Start the replay when the app starts, also hidden in the tray.
    pub replay_on_start: bool,
}

impl RecordingSettings {
    /// The defaults, with the folders under `video_dir`.
    pub fn defaults(video_dir: &Path) -> Self {
        let base = video_dir.join(VIDEO_SUBFOLDER);
        Self {
            replay_seconds: DEFAULT_REPLAY_SECONDS,
            audio_tracks: vec![
                vec!["default_output".to_owned()],
                vec!["default_input".to_owned()],
            ],
            replay_dir: base.join(REPLAY_FOLDER),
            recording_dir: base.join(RECORDING_FOLDER),
            capture: recorder::PORTAL.to_owned(),
            codec: None,
            quality: recorder::DEFAULT_QUALITY.to_owned(),
            fps: DEFAULT_FPS,
            replay_on_start: false,
        }
    }

    /// The GPU Screen Recorder configuration for these settings; `portal_token` is the token
    /// file passed with portal capture.
    pub fn replay_config(&self, portal_token: Option<&Path>) -> ReplayConfig {
        ReplayConfig {
            capture: self.capture.clone(),
            portal_token: portal_token.map(Path::to_owned),
            fps: self.fps,
            replay_seconds: self.replay_seconds,
            // mkv carries several audio tracks without surprises.
            container: "mkv".into(),
            codec: self.codec.clone(),
            quality: self.quality.clone(),
            replay_dir: self.replay_dir.clone(),
            recording_dir: self.recording_dir.clone(),
            audio: self
                .audio_tracks
                .iter()
                .map(|track| AudioTrack(track.clone()))
                .collect(),
        }
    }

    /// Every invalid field, worded for the user. Empty when the settings can be used.
    pub fn problems(&self) -> Vec<String> {
        [
            check_replay_seconds(&self.replay_seconds),
            check_audio_tracks(&self.audio_tracks),
            check_folder(&self.replay_dir),
            check_folder(&self.recording_dir),
            check_capture(&self.capture),
            check_codec(&self.codec),
            check_quality(&self.quality),
            check_fps(&self.fps),
        ]
        .into_iter()
        .filter_map(Result::err)
        .collect()
    }
}

fn check_replay_seconds(value: &u32) -> Result<(), String> {
    let range = recorder::REPLAY_SECONDS;
    range.contains(value).then_some(()).ok_or_else(|| {
        format!(
            "replay_seconds must be between {} and {}",
            range.start(),
            range.end()
        )
    })
}

fn check_fps(value: &u32) -> Result<(), String> {
    let range = recorder::FPS;
    range
        .contains(value)
        .then_some(())
        .ok_or_else(|| format!("fps must be between {} and {}", range.start(), range.end()))
}

// `&T` of the stored type: `read_field` takes `fn(&T)`.
#[allow(clippy::ptr_arg)]
fn check_quality(value: &String) -> Result<(), String> {
    recorder::QUALITIES
        .contains(&value.as_str())
        .then_some(())
        .ok_or_else(|| format!("quality must be one of {}", recorder::QUALITIES.join(", ")))
}

fn check_codec(value: &Option<String>) -> Result<(), String> {
    match value {
        None => Ok(()),
        Some(codec) if recorder::CODECS.contains(&codec.as_str()) => Ok(()),
        Some(_) => Err(format!(
            "codec must be null (automatic) or one of {}",
            recorder::CODECS.join(", ")
        )),
    }
}

/// `portal` or a monitor name: letters, digits, `-`, `_` and `.`, nothing gsr reads as syntax
/// (`|` combines sources, `;` adds options, `/` is a camera path).
// `&T` of the stored type: `read_field` takes `fn(&T)`.
#[allow(clippy::ptr_arg)]
fn check_capture(value: &String) -> Result<(), String> {
    let name_like = !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if name_like && !NOT_A_MONITOR.contains(&value.as_str()) {
        Ok(())
    } else {
        Err("capture must be \"portal\" or a monitor name".to_owned())
    }
}

/// One audio source as gsr's `-a` reads it. `|` is refused: it joins the sources of a track.
fn valid_source(source: &str) -> bool {
    if source.is_empty() || source.contains(['|', '\n', '\r']) {
        return false;
    }
    if matches!(source, "default_output" | "default_input") {
        return true;
    }
    ["device:", "app:", "app-inverse:"].iter().any(|prefix| {
        source
            .strip_prefix(prefix)
            .is_some_and(|name| !name.is_empty())
    })
}

// `&T` of the stored type: `read_field` takes `fn(&T)`.
#[allow(clippy::ptr_arg)]
fn check_audio_tracks(value: &Vec<Vec<String>>) -> Result<(), String> {
    let valid = value
        .iter()
        .all(|track| !track.is_empty() && track.iter().all(|source| valid_source(source)));
    valid.then_some(()).ok_or_else(|| {
        "audio_tracks must be a list of non-empty tracks of default_output, default_input, \
         device:<name>, app:<name> or app-inverse:<name>"
            .to_owned()
    })
}

// `&T` of the stored type: `read_field` takes `fn(&T)`.
#[allow(clippy::ptr_arg)]
fn check_folder(value: &PathBuf) -> Result<(), String> {
    value
        .is_absolute()
        .then_some(())
        .ok_or_else(|| format!("the folder \"{}\" is not an absolute path", value.display()))
}

fn check_bool(_: &bool) -> Result<(), String> {
    Ok(())
}

/// The settings as saved and as the frontend receives them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Settings {
    pub theme: Theme,
    /// Start minimised to the tray when launched by the autostart entry.
    pub start_minimised: bool,
    pub recording: RecordingSettings,
}

impl Settings {
    pub fn defaults(video_dir: &Path) -> Self {
        Self {
            theme: Theme::default(),
            start_minimised: false,
            recording: RecordingSettings::defaults(video_dir),
        }
    }
}

/// What `set_settings` receives. Without `recording` the recording settings stay as they are,
/// so a screen that edits only General never resets them.
#[derive(Debug, Clone, Deserialize)]
pub struct SettingsUpdate {
    pub theme: Theme,
    pub start_minimised: bool,
    #[serde(default)]
    pub recording: Option<RecordingSettings>,
}

/// The managed state: where the files live and the settings last loaded or saved.
pub struct SettingsState {
    /// `None` when Tauri could not resolve the config folder: the defaults apply and saving
    /// reports an error.
    file: Option<PathBuf>,
    /// The folder the default replay and recording folders live under.
    video_dir: PathBuf,
    /// `None` when Tauri could not resolve the data folder: gsr uses its own location.
    portal_token: Option<PathBuf>,
    current: Mutex<Settings>,
}

impl SettingsState {
    /// The current settings.
    pub fn current(&self) -> Settings {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The portal session token file Redline owns.
    pub fn portal_token(&self) -> Option<&Path> {
        self.portal_token.as_deref()
    }
}

/// The current recording settings, for the other backend modules: the replay service, and
/// whoever needs `replay_dir` and `recording_dir`. `None` only before [`init`] has run.
pub fn recording_settings<R: Runtime>(app: &AppHandle<R>) -> Option<RecordingSettings> {
    app.try_state::<SettingsState>()
        .map(|state| state.current().recording)
}

/// Loads the settings and registers them as managed state. Never fails: every problem is
/// reported on stderr and the defaults apply.
pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let paths = app.path();
    let file = match paths.app_config_dir() {
        Ok(dir) => Some(dir.join(SETTINGS_FILE)),
        Err(error) => {
            eprintln!("redline: settings folder not available, using the defaults: {error}");
            None
        }
    };
    // Without a video folder (and a home) the default folders are empty paths: starting a
    // replay then says there is no folder to save in.
    let video_dir = paths
        .video_dir()
        .or_else(|_| paths.home_dir().map(|home| home.join("Videos")))
        .unwrap_or_default();
    let portal_token = paths
        .app_data_dir()
        .map(|dir| dir.join(PORTAL_TOKEN_FILE))
        .ok();
    let settings = match &file {
        Some(file) => {
            let (settings, problems) = load(file, &video_dir);
            for problem in problems {
                eprintln!("redline: {problem}");
            }
            settings
        }
        None => Settings::defaults(&video_dir),
    };
    app.manage(SettingsState {
        file,
        video_dir,
        portal_token,
        current: Mutex::new(settings),
    });
}

/// The current settings; the defaults when no file has been saved yet.
#[tauri::command]
pub fn get_settings(state: State<'_, SettingsState>) -> Result<Settings, String> {
    state
        .current
        .lock()
        .map(|current| current.clone())
        .map_err(|_| UNAVAILABLE.to_owned())
}

/// Saves the settings and returns them as read back from the file. A change of the recording
/// settings restarts a running replay with them.
#[tauri::command]
pub fn set_settings<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, SettingsState>,
    settings: SettingsUpdate,
) -> Result<Settings, String> {
    let mut current = state.current.lock().map_err(|_| UNAVAILABLE.to_owned())?;
    let file = state
        .file
        .as_deref()
        .ok_or_else(|| "Could not save the settings: no settings folder.".to_owned())?;
    let saved = apply(
        file,
        &state.video_dir,
        state.portal_token.as_deref(),
        &current,
        settings,
    )?;
    let recording_changed = needs_restart(&current.recording, &saved.recording);
    *current = saved.clone();
    drop(current);
    if recording_changed {
        crate::recorder::settings_changed(&app);
    }
    Ok(saved)
}

/// Whether a running replay must restart: something that reaches GPU Screen Recorder changed.
/// `replay_on_start` matters only at the next app start, and restarting would drop the buffer.
fn needs_restart(before: &RecordingSettings, after: &RecordingSettings) -> bool {
    let mut after = after.clone();
    after.replay_on_start = before.replay_on_start;
    after != *before
}

/// Validates an update, saves it and, when `capture` changed, deletes the portal session token.
fn apply(
    file: &Path,
    video_dir: &Path,
    portal_token: Option<&Path>,
    current: &Settings,
    update: SettingsUpdate,
) -> Result<Settings, String> {
    let wanted = Settings {
        theme: update.theme,
        start_minimised: update.start_minimised,
        recording: update
            .recording
            .unwrap_or_else(|| current.recording.clone()),
    };
    let problems = wanted.recording.problems();
    if !problems.is_empty() {
        return Err(format!(
            "The recording settings were not saved: {}.",
            problems.join("; ")
        ));
    }
    let saved = save(file, &wanted, video_dir)?;
    if saved.recording.capture != current.recording.capture
        && let Some(token) = portal_token
    {
        forget_portal_session(token);
    }
    Ok(saved)
}

/// Deletes the saved portal session, so the desktop asks for the screen again.
fn forget_portal_session(token: &Path) {
    match fs::remove_file(token) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "redline: could not delete the portal session token ({})",
            error.kind()
        ),
    }
}

/// Reads one field of `fields` into `slot` when present and valid; otherwise leaves the default
/// and records why.
fn read_field<T: DeserializeOwned>(
    fields: &Map<String, Value>,
    prefix: &str,
    name: &str,
    check: fn(&T) -> Result<(), String>,
    slot: &mut T,
    problems: &mut Vec<String>,
) {
    let Some(value) = fields.get(name) else {
        return;
    };
    let read = T::deserialize(value)
        .map_err(|_| format!("{name} has the wrong type"))
        .and_then(|candidate| check(&candidate).map(|()| candidate));
    match read {
        Ok(candidate) => *slot = candidate,
        Err(reason) => problems.push(format!(
            "{SETTINGS_FILE}: {prefix}{reason}, using the default for {name}"
        )),
    }
}

/// The `recording` object, field by field over the defaults.
fn load_recording(
    value: &Value,
    video_dir: &Path,
    problems: &mut Vec<String>,
) -> RecordingSettings {
    let mut recording = RecordingSettings::defaults(video_dir);
    let Value::Object(fields) = value else {
        problems.push(format!(
            "{SETTINGS_FILE}: recording is not an object, using the defaults"
        ));
        return recording;
    };
    let p = "recording.";
    let r = &mut recording;
    read_field(
        fields,
        p,
        "replay_seconds",
        check_replay_seconds,
        &mut r.replay_seconds,
        problems,
    );
    read_field(
        fields,
        p,
        "audio_tracks",
        check_audio_tracks,
        &mut r.audio_tracks,
        problems,
    );
    read_field(
        fields,
        p,
        "replay_dir",
        check_folder,
        &mut r.replay_dir,
        problems,
    );
    read_field(
        fields,
        p,
        "recording_dir",
        check_folder,
        &mut r.recording_dir,
        problems,
    );
    read_field(
        fields,
        p,
        "capture",
        check_capture,
        &mut r.capture,
        problems,
    );
    read_field(fields, p, "codec", check_codec, &mut r.codec, problems);
    read_field(
        fields,
        p,
        "quality",
        check_quality,
        &mut r.quality,
        problems,
    );
    read_field(fields, p, "fps", check_fps, &mut r.fps, problems);
    read_field(
        fields,
        p,
        "replay_on_start",
        check_bool,
        &mut r.replay_on_start,
        problems,
    );
    recording
}

/// Reads the settings file. Returns the settings to use and the problems found, already worded
/// for stderr; a missing file is not a problem.
fn load(file: &Path, video_dir: &Path) -> (Settings, Vec<String>) {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return (Settings::defaults(video_dir), Vec::new());
        }
        Err(error) => {
            return (
                Settings::defaults(video_dir),
                vec![format!(
                    "could not read {SETTINGS_FILE} ({}), using the defaults",
                    error.kind()
                )],
            );
        }
    };
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(&text) else {
        return (
            Settings::defaults(video_dir),
            vec![format!(
                "{SETTINGS_FILE} is not a valid JSON object, using the defaults"
            )],
        );
    };

    let mut problems = Vec::new();
    let mut settings = Settings::defaults(video_dir);
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
    if let Some(value) = fields.get("recording") {
        settings.recording = load_recording(value, video_dir, &mut problems);
    }
    (settings, problems)
}

/// Writes the settings atomically and reads them back.
fn save(file: &Path, settings: &Settings, video_dir: &Path) -> Result<Settings, String> {
    write_atomically(file, settings)
        .map_err(|error| format!("Could not save the settings ({}).", error.kind()))?;
    let (saved, problems) = load(file, video_dir);
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
    // One temporary file per process: two instances saving at once never write into the same
    // file (single-instance handling comes later, D8).
    let tmp = file.with_extension(format!("json.tmp.{}", std::process::id()));
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
    use crate::test_support::TempDir;

    const VIDEOS: &str = "/videos";

    fn videos() -> &'static Path {
        Path::new(VIDEOS)
    }

    fn defaults() -> Settings {
        Settings::defaults(videos())
    }

    /// A fresh folder under the system temp directory, never the real config folder.
    /// Removed when the test ends, also when it fails.
    fn temp_dir(name: &str) -> TempDir {
        TempDir::new("settings", name)
    }

    fn update(settings: &Settings) -> SettingsUpdate {
        SettingsUpdate {
            theme: settings.theme,
            start_minimised: settings.start_minimised,
            recording: Some(settings.recording.clone()),
        }
    }

    #[test]
    fn a_missing_file_gives_the_defaults_without_problems() {
        let dir = temp_dir("missing");
        let (settings, problems) = load(&dir.join(SETTINGS_FILE), videos());
        assert_eq!(settings.theme, Theme::System);
        assert!(!settings.start_minimised);
        assert_eq!(
            settings.recording,
            RecordingSettings {
                replay_seconds: 60,
                audio_tracks: vec![vec!["default_output".into()], vec!["default_input".into()]],
                replay_dir: PathBuf::from("/videos/Redline/Replays"),
                recording_dir: PathBuf::from("/videos/Redline/Recordings"),
                capture: "portal".into(),
                codec: None,
                quality: "very_high".into(),
                fps: 60,
                replay_on_start: false,
            }
        );
        assert!(problems.is_empty());
        assert!(settings.recording.problems().is_empty());
    }

    #[test]
    fn invalid_json_gives_the_defaults_and_a_problem() {
        let dir = temp_dir("invalid");
        let file = dir.join(SETTINGS_FILE);
        for text in ["{ not json", "[]", "\"dark\"", ""] {
            fs::write(&file, text).unwrap();
            let (settings, problems) = load(&file, videos());
            assert_eq!(settings, defaults(), "{text:?}");
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
        let (settings, problems) = load(&file, videos());
        assert_eq!(settings, defaults());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn an_unknown_theme_falls_back_to_system_and_keeps_the_other_fields() {
        let dir = temp_dir("unknown-theme");
        let file = dir.join(SETTINGS_FILE);
        fs::write(&file, r#"{"theme": "purple", "start_minimised": true}"#).unwrap();
        let (settings, problems) = load(&file, videos());
        assert_eq!(settings.theme, Theme::System);
        assert!(settings.start_minimised);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("unknown theme"));
    }

    #[test]
    fn a_wrong_type_falls_back_per_field_and_missing_fields_are_silent() {
        let dir = temp_dir("fields");
        let file = dir.join(SETTINGS_FILE);
        fs::write(&file, r#"{"theme": "dark", "start_minimised": "yes"}"#).unwrap();
        let (settings, problems) = load(&file, videos());
        assert_eq!(settings.theme, Theme::Dark);
        assert!(!settings.start_minimised);
        assert_eq!(problems.len(), 1);

        fs::write(&file, r#"{"theme": "light", "future_option": 3}"#).unwrap();
        let (settings, problems) = load(&file, videos());
        assert_eq!(settings.theme, Theme::Light);
        assert!(!settings.start_minimised);
        assert_eq!(settings.recording, defaults().recording);
        assert!(problems.is_empty());
    }

    #[test]
    fn invalid_recording_fields_fall_back_one_by_one() {
        let dir = temp_dir("recording-fields");
        let file = dir.join(SETTINGS_FILE);
        fs::write(
            &file,
            r#"{"theme": "dark", "recording": {
                "replay_seconds": 1,
                "audio_tracks": [["default_output", "app:Discord"], []],
                "replay_dir": "relative/replays",
                "recording_dir": "/games/clips",
                "capture": "DP-2",
                "codec": "mpeg2",
                "quality": "ultra",
                "fps": 0,
                "replay_on_start": "yes"
            }}"#,
        )
        .unwrap();
        let (settings, problems) = load(&file, videos());
        let wanted = RecordingSettings {
            recording_dir: PathBuf::from("/games/clips"),
            capture: "DP-2".into(),
            quality: "ultra".into(),
            ..RecordingSettings::defaults(videos())
        };
        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(settings.recording, wanted);
        // replay_seconds, audio_tracks, replay_dir, codec, fps, replay_on_start.
        assert_eq!(problems.len(), 6, "{problems:?}");
        assert!(problems.iter().all(|p| p.contains("recording.")));

        fs::write(&file, r#"{"recording": 7}"#).unwrap();
        let (settings, problems) = load(&file, videos());
        assert_eq!(settings, defaults());
        assert_eq!(problems.len(), 1);

        // A partial object: the missing fields take their defaults silently.
        fs::write(&file, r#"{"recording": {"fps": 144, "codec": "av1"}}"#).unwrap();
        let (settings, problems) = load(&file, videos());
        assert_eq!(
            (settings.recording.fps, settings.recording.codec.as_deref()),
            (144, Some("av1"))
        );
        assert_eq!(settings.recording.replay_seconds, 60);
        assert!(problems.is_empty());
    }

    #[test]
    fn the_allowed_values_follow_gpu_screen_recorder() {
        let valid = defaults().recording;
        let with = |change: fn(&mut RecordingSettings)| {
            let mut settings = valid.clone();
            change(&mut settings);
            settings.problems()
        };
        assert!(with(|s| s.replay_seconds = 2).is_empty());
        assert!(with(|s| s.replay_seconds = 86_400).is_empty());
        assert_eq!(with(|s| s.replay_seconds = 86_401).len(), 1);
        assert_eq!(with(|s| s.fps = 0).len(), 1);
        assert!(with(|s| s.codec = Some("hevc".into())).is_empty());
        assert_eq!(with(|s| s.codec = Some("auto".into())).len(), 1);
        assert_eq!(with(|s| s.quality = "5000".into()).len(), 1);
        assert!(with(|s| s.capture = "HDMI-A-1".into()).is_empty());
        for capture in [
            "",
            "region",
            "focused",
            "DP-1|DP-2",
            "screen;x=5",
            "/dev/video0",
        ] {
            let mut settings = valid.clone();
            settings.capture = capture.into();
            assert_eq!(settings.problems().len(), 1, "{capture:?}");
        }
        assert!(
            with(|s| {
                s.audio_tracks = vec![vec![
                    "device:alsa_input.usb-mic".into(),
                    "app-inverse:Firefox".into(),
                ]]
            })
            .is_empty()
        );
        assert!(with(|s| s.audio_tracks = Vec::new()).is_empty());
        for source in ["", "app:", "bare-device", "app:a|app:b", "device:"] {
            let mut settings = valid.clone();
            settings.audio_tracks = vec![vec![source.into()]];
            assert_eq!(settings.problems().len(), 1, "{source:?}");
        }
    }

    #[test]
    fn a_round_trip_creates_the_folder_and_reads_back_what_was_saved() {
        let dir = temp_dir("round-trip");
        let file = dir.join("nested/config").join(SETTINGS_FILE);
        let mut custom = defaults();
        custom.theme = Theme::Dark;
        custom.start_minimised = true;
        custom.recording = RecordingSettings {
            replay_seconds: 120,
            audio_tracks: vec![vec!["default_output".into(), "app:Discord".into()]],
            replay_dir: PathBuf::from("/games/replays"),
            recording_dir: PathBuf::from("/games/recordings"),
            capture: "DP-2".into(),
            codec: Some("av1".into()),
            quality: "high".into(),
            fps: 144,
            replay_on_start: true,
        };
        for wanted in [custom, defaults()] {
            assert_eq!(save(&file, &wanted, videos()), Ok(wanted.clone()));
            assert_eq!(load(&file, videos()), (wanted, Vec::new()));
        }
        // The temporary file does not survive a successful write.
        assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn saving_replaces_a_damaged_file() {
        let dir = temp_dir("replace-damaged");
        let file = dir.join(SETTINGS_FILE);
        fs::write(&file, "{ damaged").unwrap();
        let mut wanted = defaults();
        wanted.theme = Theme::Dark;
        assert_eq!(save(&file, &wanted, videos()), Ok(wanted));
    }

    #[test]
    fn the_file_uses_the_documented_names_and_values() {
        let dir = temp_dir("shape");
        let file = dir.join(SETTINGS_FILE);
        let mut settings = defaults();
        settings.theme = Theme::Light;
        settings.start_minimised = true;
        save(&file, &settings, videos()).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "theme": "light",
                "start_minimised": true,
                "recording": {
                    "replay_seconds": 60,
                    "audio_tracks": [["default_output"], ["default_input"]],
                    "replay_dir": "/videos/Redline/Replays",
                    "recording_dir": "/videos/Redline/Recordings",
                    "capture": "portal",
                    "codec": null,
                    "quality": "very_high",
                    "fps": 60,
                    "replay_on_start": false,
                },
            })
        );
    }

    #[test]
    fn a_failed_write_is_an_error_and_touches_nothing_else() {
        let dir = temp_dir("failed-write");
        // The parent "folder" is a regular file: creating the folder fails.
        let blocker = dir.join("blocker");
        fs::write(&blocker, "").unwrap();
        let file = blocker.join(SETTINGS_FILE);
        assert!(save(&file, &defaults(), videos()).is_err());
        assert_eq!(fs::read_to_string(&blocker).unwrap(), "");
    }

    #[test]
    fn an_update_without_recording_keeps_the_recording_settings() {
        let dir = temp_dir("general-only");
        let file = dir.join(SETTINGS_FILE);
        let mut current = defaults();
        current.recording.fps = 144;
        // As General sends it today: theme and start_minimised only.
        let update: SettingsUpdate =
            serde_json::from_value(serde_json::json!({"theme": "dark", "start_minimised": false}))
                .unwrap();
        let saved = apply(&file, videos(), None, &current, update).unwrap();
        assert_eq!(saved.theme, Theme::Dark);
        assert_eq!(saved.recording, current.recording);
    }

    #[test]
    fn an_invalid_update_is_refused_with_every_reason_and_saves_nothing() {
        let dir = temp_dir("refused");
        let file = dir.join(SETTINGS_FILE);
        let mut wanted = defaults();
        wanted.recording.fps = 0;
        wanted.recording.quality = "best".into();
        let error = apply(&file, videos(), None, &defaults(), update(&wanted)).unwrap_err();
        assert!(
            error.contains("fps") && error.contains("quality"),
            "{error}"
        );
        assert!(!file.exists());
    }

    #[test]
    fn changing_capture_deletes_the_portal_session_token() {
        let dir = temp_dir("token");
        let file = dir.join(SETTINGS_FILE);
        let token = dir.join("data").join(PORTAL_TOKEN_FILE);
        fs::create_dir_all(token.parent().unwrap()).unwrap();
        let current = defaults();

        // Another field changes: the token stays.
        fs::write(&token, "token").unwrap();
        let mut wanted = current.clone();
        wanted.recording.fps = 30;
        let saved = apply(&file, videos(), Some(&token), &current, update(&wanted)).unwrap();
        assert!(token.exists());

        // The screen changes: the desktop must ask again.
        let mut wanted = saved.clone();
        wanted.recording.capture = "DP-2".into();
        let saved = apply(&file, videos(), Some(&token), &saved, update(&wanted)).unwrap();
        assert!(!token.exists());
        assert_eq!(saved.recording.capture, "DP-2");

        // No token to delete is not an error.
        let mut wanted = saved.clone();
        wanted.recording.capture = "portal".into();
        assert!(apply(&file, videos(), Some(&token), &saved, update(&wanted)).is_ok());
    }

    #[test]
    fn only_changes_that_reach_the_process_restart_the_replay() {
        let before = defaults().recording;
        let mut after = before.clone();
        after.replay_on_start = true;
        assert!(!needs_restart(&before, &after));
        after.replay_seconds = 30;
        assert!(needs_restart(&before, &after));
        let mut after = before.clone();
        after.capture = "DP-2".into();
        assert!(needs_restart(&before, &after));
        assert!(!needs_restart(&before, &before));
    }

    #[test]
    fn the_replay_config_follows_the_settings() {
        let mut recording = defaults().recording;
        recording.codec = Some("hevc".into());
        recording.audio_tracks = vec![vec!["default_output".into(), "app:Discord".into()]];
        let token = Path::new("/data/portal-session-token");
        let config = recording.replay_config(Some(token));
        assert_eq!(config.capture, "portal");
        assert_eq!(config.portal_token.as_deref(), Some(token));
        assert_eq!((config.fps, config.replay_seconds), (60, 60));
        assert_eq!(config.codec.as_deref(), Some("hevc"));
        assert_eq!(config.quality, "very_high");
        assert_eq!(config.container, "mkv");
        assert_eq!(config.replay_dir, Path::new("/videos/Redline/Replays"));
        assert_eq!(
            config.recording_dir,
            Path::new("/videos/Redline/Recordings")
        );
        let tracks: Vec<_> = config.audio.iter().map(|track| track.0.clone()).collect();
        assert_eq!(tracks, [vec!["default_output", "app:Discord"]]);
    }
}
