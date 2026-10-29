//! The instant replay service: at most one GPU Screen Recorder replay session for the whole app,
//! living in the app process so it keeps running while the window is hidden in the tray (D8).
//!
//! The process itself is `redline-core::recorder`: this module owns the session, turns what
//! happens to it into a [`RecorderStatus`] and two events, and exposes the commands.
//!
//! - `recorder-state` carries the [`RecorderStatus`] on every change, including an unexpected exit
//!   of the process (`replay: "error"`, the reason in `error`).
//! - `file-saved` carries a [`FileSaved`] for every file gsr reports on stdout. Only the file
//!   name leaves the backend, never the path.
//!
//! Replay states: `off` → `starting` (process launched; with the portal the desktop is asking
//! for a screen) → `on` (gsr answers on its IPC socket, which it serves only once capture runs).
//! Any exit we did not ask for → `error`; stopping → `off`.
//!
//! Regular recordings ([`Recorder::toggle_recording`], `recording` in the status) run in one of
//! two ways, and there is never more than one gsr process ([`ServiceState::running`]):
//! - replay on: the replay process records into `recording_dir` (`start-replay-recording` /
//!   `stop-replay-recording` over its IPC socket);
//! - replay off (or in error): a separate gsr process records one file into `recording_dir` and
//!   is stopped like the replay (SIGINT to our own gsr). The replay cannot start while it runs.
//!
//! Stopping the replay, a settings change that restarts it, and quitting all stop the process
//! with SIGINT, which finishes the recording and keeps the file (gsr prints its path, so
//! `file-saved` follows).
//!
//! Extension points for the issues built on this one:
//! - settings (#29, done): [`Environment::config`] is the single source of the [`ReplayConfig`],
//!   built from `settings::RecordingSettings`; [`settings_changed`] restarts a running replay.
//! - gallery and notifications (#31): subscribe to [`Events::file_saved`]; the folders come from
//!   `settings::recording_settings`.
//! - command-line actions (#32): call [`Recorder::start`], [`Recorder::stop`], [`Recorder::save`],
//!   [`Recorder::toggle_recording`] from the single-instance callback, as the commands do.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redline_core::recorder::{
    self, Launcher, Mode, Output, ReadyProbe, ReplayConfig, ReplaySession,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::settings::{self, SettingsState};

pub const STATE_EVENT: &str = "recorder-state";
pub const FILE_SAVED_EVENT: &str = "file-saved";
/// How often the supervisor looks at the process and at the IPC socket.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Saving waits for gsr to write the file.
const SAVE_TIMEOUT: Duration = Duration::from_secs(30);

pub const MISSING: &str = "GPU Screen Recorder is not installed. Install it from Flathub \
    (com.dec05eba.gpu_screen_recorder) or from your distribution's packages to use instant replay.";
const NOT_RUNNING: &str = "The replay is off: start it first.";
const STILL_STARTING: &str =
    "The replay is still starting: GPU Screen Recorder is waiting for the screen to capture.";
const NO_VIDEO_FOLDER: &str = "No video folder to save replays in.";
const RECORDING_RUNS: &str =
    "A recording is running without the replay: stop it before starting the replay.";
/// gsr's stdout lines (6.1.6) for a regular recording that did not start or was not saved.
const GSR_RECORDING_FAILURES: [&str; 2] = ["Failed to start recording", "Failed to save recording"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayState {
    Off,
    Starting,
    On,
    Error,
}

/// The recorder as the interface sees it, from `recorder_status` and the `recorder-state` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecorderStatus {
    /// GPU Screen Recorder was found; false until the first detection has run.
    pub available: bool,
    /// When not available: what to install.
    pub unavailable_reason: Option<String>,
    pub replay: ReplayState,
    /// A regular recording is running: in the replay process, or in its own process when the
    /// replay is off. With the portal it is true from the start, while the desktop asks for the
    /// screen.
    pub recording: bool,
    /// The replay length: of the running replay, otherwise of the next one.
    pub replay_seconds: u32,
    /// Why the replay stopped, or why the last action failed.
    pub error: Option<String>,
    /// A notice that blocks nothing, such as an unverified GPU Screen Recorder version.
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Replay,
    Recording,
}

/// The payload of `file-saved`: the file name only, never its path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileSaved {
    pub kind: FileKind,
    pub file_name: String,
    pub saved_at_ms: u64,
}

/// Where the service reports: the app's webviews, or a test's collector.
pub trait Events: Send + Sync + 'static {
    fn state(&self, status: &RecorderStatus);
    fn file_saved(&self, file: &FileSaved);
}

struct AppEvents<R: Runtime>(AppHandle<R>);

impl<R: Runtime> Events for AppEvents<R> {
    fn state(&self, status: &RecorderStatus) {
        if let Err(error) = self.0.emit(STATE_EVENT, status) {
            eprintln!("redline: could not send {STATE_EVENT} ({error})");
        }
    }

    fn file_saved(&self, file: &FileSaved) {
        if let Err(error) = self.0.emit(FILE_SAVED_EVENT, file) {
            eprintln!("redline: could not send {FILE_SAVED_EVENT} ({error})");
        }
        // The desktop notification (#31): every saved file, also with the window hidden.
        crate::gallery::notify_saved(file);
    }
}

/// Whether GPU Screen Recorder can be used.
#[derive(Debug, Clone)]
pub enum Availability {
    Ready {
        launcher: Launcher,
        /// Set when the version is not the verified one.
        warning: Option<String>,
    },
    Missing(String),
}

/// Finds GPU Screen Recorder and checks its version (runs it: up to half a second).
pub fn detect() -> Availability {
    match Launcher::detect() {
        Ok(launcher) => {
            let version = launcher.version().ok();
            Availability::Ready {
                warning: recorder::version_notice(version.as_deref()),
                launcher,
            }
        }
        Err(_) => Availability::Missing(MISSING.to_owned()),
    }
}

type Source<T> = Box<dyn Fn() -> Result<T, String> + Send + Sync>;

/// What the service needs from its surroundings; tests pass a fake process.
pub struct Environment {
    pub detect: Box<dyn Fn() -> Availability + Send + Sync>,
    /// The settings of the next replay: the recording settings.
    pub config: Source<ReplayConfig>,
    /// Where the session's IPC socket goes; private to this app instance.
    pub socket: Result<PathBuf, String>,
}

/// The running session.
struct Running {
    session: ReplaySession,
    socket: PathBuf,
    /// The replay, or a recording without the replay.
    mode: Mode,
}

impl Running {
    fn is_replay(&self) -> bool {
        self.mode == Mode::Replay
    }
}

struct ServiceState {
    availability: Option<Availability>,
    status: RecorderStatus,
    /// The only gsr process: the replay or a recording without it, never both.
    running: Option<Running>,
    /// Bumped by every start and stop: a thread of an older session finds a different value and
    /// leaves the state alone.
    generation: u64,
}

struct Inner {
    env: Environment,
    events: Box<dyn Events>,
    /// Serialises start, stop and detection, which are slow and must not interleave. Never
    /// taken while `state` is held.
    control: Mutex<()>,
    state: Mutex<ServiceState>,
}

/// The managed service. Cheap to clone: every clone is the same service.
#[derive(Clone)]
pub struct Recorder {
    inner: Arc<Inner>,
}

impl Recorder {
    pub fn new(env: Environment, events: impl Events) -> Self {
        let replay_seconds = (env.config)().map_or(0, |config| config.replay_seconds);
        let status = RecorderStatus {
            available: false,
            unavailable_reason: None,
            replay: ReplayState::Off,
            recording: false,
            replay_seconds,
            error: None,
            warning: None,
        };
        Self {
            inner: Arc::new(Inner {
                env,
                events: Box::new(events),
                control: Mutex::new(()),
                state: Mutex::new(ServiceState {
                    availability: None,
                    status,
                    running: None,
                    generation: 0,
                }),
            }),
        }
    }

    /// The current status; runs the first detection if it has not happened yet.
    pub fn status(&self) -> RecorderStatus {
        if self.state().availability.is_none() {
            let _control = lock(&self.inner.control);
            // Checked again: the background detection may have finished meanwhile.
            if self.state().availability.is_none() {
                self.refresh_availability();
            }
        }
        self.state().status.clone()
    }

    /// Looks for GPU Screen Recorder again and publishes the result.
    pub fn detect(&self) {
        let _control = lock(&self.inner.control);
        self.refresh_availability();
    }

    /// Starts the replay; nothing happens when it is already starting or on. Fails with the
    /// sentence that says what to install when GPU Screen Recorder is missing.
    pub fn start(&self) -> Result<RecorderStatus, String> {
        let _control = lock(&self.inner.control);
        self.start_locked()
    }

    /// [`Recorder::start`] with `control` already held.
    fn start_locked(&self) -> Result<RecorderStatus, String> {
        // Bound first: a guard in the match scrutinee would live through the arms.
        let running = self.state().running.as_ref().map(Running::is_replay);
        match running {
            Some(true) => return Ok(self.state().status.clone()),
            Some(false) => return Err(RECORDING_RUNS.to_owned()),
            None => {}
        }
        let launcher = self.launcher_locked()?;
        let config = (self.inner.env.config)().map_err(|error| self.fail(error))?;
        let socket = self
            .inner
            .env
            .socket
            .clone()
            .map_err(|error| self.fail(error))?;

        // `starting` is published before the process exists, so that nothing it prints can
        // arrive before it and be overwritten.
        let mut state = self.state();
        state.generation += 1;
        let generation = state.generation;
        state.status.replay = ReplayState::Starting;
        state.status.recording = false;
        state.status.replay_seconds = config.replay_seconds;
        state.status.error = None;
        let status = state.status.clone();
        drop(state);
        self.inner.events.state(&status);

        let output = self.output_handler(generation, &config);
        let session =
            ReplaySession::start(&launcher, &config, &socket, output).map_err(|error| {
                self.fail(format!("Could not start GPU Screen Recorder: {error:#}"))
            })?;
        let mut state = self.state();
        state.running = Some(Running {
            session,
            socket: socket.clone(),
            mode: Mode::Replay,
        });
        let status = state.status.clone();
        drop(state);
        self.supervise(generation, socket);
        Ok(status)
    }

    /// Stops the replay (SIGINT to our own process, SIGKILL to our own tree if it hangs) and
    /// clears a previous error. Blocks until the process has exited.
    pub fn stop(&self) -> Result<RecorderStatus, String> {
        let _control = lock(&self.inner.control);
        self.stop_locked()
    }

    /// [`Recorder::stop`] with `control` already held. A running replay is always stopped,
    /// whatever the last detection said: stopping needs only our own process tree, not the
    /// binary; a recording in it ends and is kept. With nothing running, a missing GPU Screen
    /// Recorder is the answer. A recording without the replay is left alone.
    fn stop_locked(&self) -> Result<RecorderStatus, String> {
        let mut state = self.state();
        match state.running.as_ref().map(Running::is_replay) {
            Some(false) => return Ok(state.status.clone()),
            Some(true) => {}
            None => {
                drop(state);
                self.ensure_available()?;
                state = self.state();
            }
        }
        state.generation += 1;
        let running = state.running.take();
        drop(state);

        let stopped = running.map(|mut running| running.session.stop());
        let mut state = self.state();
        state.status.replay = ReplayState::Off;
        state.status.recording = false;
        state.status.error = match stopped {
            Some(Err(error)) => Some(format!("Could not stop GPU Screen Recorder: {error:#}")),
            _ => None,
        };
        let status = state.status.clone();
        drop(state);
        self.inner.events.state(&status);
        match &status.error {
            Some(error) => Err(error.clone()),
            None => Ok(status),
        }
    }

    /// Saves the replay. Returns once gsr has written the file; the `file-saved` event carries
    /// its name.
    pub fn save(&self) -> Result<(), String> {
        self.ensure_available()?;
        let socket = {
            let state = self.state();
            match (&state.running, state.status.replay) {
                (Some(running), ReplayState::On) => running.socket.clone(),
                (Some(running), _) if running.is_replay() => {
                    return Err(STILL_STARTING.to_owned());
                }
                _ => return Err(NOT_RUNNING.to_owned()),
            }
        };
        recorder::save_replay(&socket, SAVE_TIMEOUT)
            .map(|_| ())
            .map_err(|error| format!("Could not save the replay: {error:#}"))
    }

    /// Applies changed settings: a running replay (starting or on) restarts with them; otherwise
    /// the status shows the length of the next replay.
    pub fn reconfigure(&self) -> Result<RecorderStatus, String> {
        let _control = lock(&self.inner.control);
        if self
            .state()
            .running
            .as_ref()
            .is_some_and(Running::is_replay)
        {
            self.stop_locked()?;
            return self.start_locked();
        }
        let Ok(config) = (self.inner.env.config)() else {
            return Ok(self.state().status.clone());
        };
        let mut state = self.state();
        let changed = state.status.replay_seconds != config.replay_seconds;
        state.status.replay_seconds = config.replay_seconds;
        let status = state.status.clone();
        drop(state);
        if changed {
            self.inner.events.state(&status);
        }
        Ok(status)
    }

    /// The GPU Screen Recorder to run, for the listing commands; the sentence that says what to
    /// install when it is missing.
    pub fn launcher(&self) -> Result<Launcher, String> {
        let _control = lock(&self.inner.control);
        self.launcher_locked()
    }

    /// [`Recorder::launcher`] with `control` already held. Detected again when missing:
    /// installing it does not require a restart.
    fn launcher_locked(&self) -> Result<Launcher, String> {
        // Bound first: a guard in the match scrutinee would live through the arms.
        let known = self.state().availability.clone();
        match known {
            Some(Availability::Ready { launcher, .. }) => Ok(launcher),
            _ => match self.refresh_availability() {
                Availability::Ready { launcher, .. } => Ok(launcher),
                Availability::Missing(reason) => Err(reason),
            },
        }
    }

    /// Starts a regular recording, or stops the one running; the returned status says which.
    /// With the replay on, the replay process records; with the replay off (or in error), a
    /// separate process does. Fails with the sentence that says what to install when GPU Screen
    /// Recorder is missing, and while the replay is still starting.
    pub fn toggle_recording(&self) -> Result<RecorderStatus, String> {
        let _control = lock(&self.inner.control);
        let state = self.state();
        let recording = state.status.recording;
        let replay = state.status.replay;
        let running = state
            .running
            .as_ref()
            .map(|running| (running.is_replay(), running.socket.clone()));
        drop(state);
        match (running, recording) {
            (Some((false, _)), _) => self.stop_recording_process(),
            (Some((true, socket)), true) => self.stop_replay_recording(&socket),
            (Some((true, _)), false) if replay != ReplayState::On => Err(STILL_STARTING.to_owned()),
            (Some((true, socket)), false) => self.start_replay_recording(&socket),
            (None, _) => self.start_recording_process(),
        }
    }

    /// Asks the replay process to record; gsr reports a failure later on stdout.
    fn start_replay_recording(&self, socket: &Path) -> Result<RecorderStatus, String> {
        recorder::start_replay_recording(socket)
            .map_err(|error| format!("Could not start the recording: {error:#}"))?;
        Ok(self.set_recording(true, None))
    }

    /// Asks the replay process to stop recording; returns once the file is written. A refusal
    /// (no recording was running after all) also ends the `recording` state.
    fn stop_replay_recording(&self, socket: &Path) -> Result<RecorderStatus, String> {
        match recorder::stop_replay_recording(socket, SAVE_TIMEOUT) {
            Ok(_) => Ok(self.set_recording(false, None)),
            Err(error) => {
                let error = format!("Could not stop the recording: {error:#}");
                self.set_recording(false, Some(error.clone()));
                Err(error)
            }
        }
    }

    /// Starts gsr recording one file into `recording_dir`, without the replay.
    fn start_recording_process(&self) -> Result<RecorderStatus, String> {
        let launcher = self.launcher_locked()?;
        let fail = |error: String| {
            self.set_recording(false, Some(error.clone()));
            error
        };
        let config = (self.inner.env.config)().map_err(fail)?;
        let socket = self.inner.env.socket.clone().map_err(fail)?;
        let file =
            recorder::recording_file(&config.recording_dir, &config.container, SystemTime::now());

        let mut state = self.state();
        state.generation += 1;
        let generation = state.generation;
        drop(state);
        let output = self.output_handler(generation, &config);
        let session = ReplaySession::start_recording(&launcher, &config, &file, &socket, output)
            .map_err(|error| fail(format!("Could not start the recording: {error:#}")))?;
        let mut state = self.state();
        state.running = Some(Running {
            session,
            socket: socket.clone(),
            mode: Mode::Recording(file),
        });
        // A previous replay error is acknowledged by this action, as by a new start.
        if state.status.replay == ReplayState::Error {
            state.status.replay = ReplayState::Off;
        }
        drop(state);
        let status = self.set_recording(true, None);
        self.supervise(generation, socket);
        Ok(status)
    }

    /// Stops the recording process (SIGINT): gsr closes the file and prints its path.
    fn stop_recording_process(&self) -> Result<RecorderStatus, String> {
        let mut state = self.state();
        state.generation += 1;
        let running = state.running.take();
        drop(state);
        let error = match running.map(|mut running| running.session.stop()) {
            Some(Err(error)) => Some(format!("Could not stop the recording: {error:#}")),
            _ => None,
        };
        let status = self.set_recording(false, error);
        match &status.error {
            Some(error) => Err(error.clone()),
            None => Ok(status),
        }
    }

    /// Sets `recording` and `error`, and publishes the status.
    fn set_recording(&self, recording: bool, error: Option<String>) -> RecorderStatus {
        let mut state = self.state();
        state.status.recording = recording;
        state.status.error = error;
        let status = state.status.clone();
        drop(state);
        self.inner.events.state(&status);
        status
    }

    /// Stops whatever runs when the app exits; a recording in progress is finished and kept.
    pub fn shutdown(&self) {
        // Under `control`: a start in flight finishes first and its session is stopped too.
        let _control = lock(&self.inner.control);
        let running = self.state().running.as_ref().map(Running::is_replay);
        match running {
            Some(true) => {
                let _ = self.stop_locked();
            }
            Some(false) => {
                let _ = self.stop_recording_process();
            }
            None => {}
        }
    }

    fn state(&self) -> MutexGuard<'_, ServiceState> {
        lock(&self.inner.state)
    }

    fn ensure_available(&self) -> Result<(), String> {
        let availability = self.state().availability.clone();
        match availability {
            Some(Availability::Missing(reason)) => Err(reason),
            _ => Ok(()),
        }
    }

    /// Runs detection (with `control` held by the caller) and publishes it when it changed.
    fn refresh_availability(&self) -> Availability {
        let availability = (self.inner.env.detect)();
        let mut state = self.state();
        let before = state.status.clone();
        match &availability {
            Availability::Ready { warning, .. } => {
                state.status.available = true;
                state.status.unavailable_reason = None;
                state.status.warning = warning.clone();
            }
            Availability::Missing(reason) => {
                state.status.available = false;
                state.status.unavailable_reason = Some(reason.clone());
                state.status.warning = None;
            }
        }
        state.availability = Some(availability.clone());
        let status = state.status.clone();
        drop(state);
        if status != before {
            self.inner.events.state(&status);
        }
        availability
    }

    /// Records a failed start as the replay's error and returns the message.
    fn fail(&self, error: String) -> String {
        let mut state = self.state();
        state.status.replay = ReplayState::Error;
        state.status.error = Some(error.clone());
        let status = state.status.clone();
        drop(state);
        self.inner.events.state(&status);
        error
    }

    /// Turns gsr's stdout into `file-saved` events and failed saves into the status' error.
    fn output_handler(
        &self,
        generation: u64,
        config: &ReplayConfig,
    ) -> impl FnMut(Output) + Send + 'static {
        let inner = Arc::clone(&self.inner);
        let replay_dir = config.replay_dir.clone();
        let recording_dir = config.recording_dir.clone();
        move |output| match output {
            Output::Saved(path) => {
                if let Some(file) = file_saved(&path, &replay_dir, &recording_dir, now_ms()) {
                    inner.events.file_saved(&file);
                }
            }
            Output::Failed(message) => {
                let mut state = lock(&inner.state);
                if state.generation != generation {
                    return;
                }
                if GSR_RECORDING_FAILURES.contains(&message.as_str()) {
                    state.status.recording = false;
                }
                state.status.error = Some(format!("GPU Screen Recorder: {message}"));
                let status = state.status.clone();
                drop(state);
                inner.events.state(&status);
            }
        }
    }

    /// Watches the session of `generation`: `starting` becomes `on` when gsr answers on its IPC
    /// socket, and an exit nobody asked for becomes `error`.
    fn supervise(&self, generation: u64, socket: PathBuf) {
        let inner = Arc::clone(&self.inner);
        let spawned = thread::Builder::new()
            .name("redline-recorder-supervisor".into())
            .spawn(move || {
                // A recording without the replay has no readiness to report.
                let replay = lock(&inner.state)
                    .running
                    .as_ref()
                    .is_some_and(Running::is_replay);
                let mut probe = replay.then(|| ReadyProbe::new(&socket));
                loop {
                    thread::sleep(POLL_INTERVAL);
                    // Polled outside the lock: it never blocks, but it does I/O.
                    let ready = probe.as_mut().is_some_and(ReadyProbe::poll);
                    let mut state = lock(&inner.state);
                    if state.generation != generation {
                        return;
                    }
                    let Some(running) = state.running.as_mut() else {
                        return;
                    };
                    let exited = match running.session.try_exit() {
                        Ok(exited) => exited,
                        Err(error) => {
                            eprintln!("redline: cannot read the recorder process state ({error})");
                            None
                        }
                    };
                    let replay = running.is_replay();
                    if let Some(exit) = exited {
                        let message = exit_message(exit, running.session.last_error());
                        state.running = None;
                        if replay {
                            state.status.replay = ReplayState::Error;
                        }
                        state.status.recording = false;
                        state.status.error = Some(message);
                    } else if ready && replay && state.status.replay == ReplayState::Starting {
                        probe = None;
                        state.status.replay = ReplayState::On;
                    } else {
                        continue;
                    }
                    let status = state.status.clone();
                    let done = state.running.is_none();
                    drop(state);
                    inner.events.state(&status);
                    if done {
                        return;
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("redline: the replay supervisor did not start ({error})");
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take the recorder down: every write leaves the state whole.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The `file-saved` payload for a path gsr reported: a recording when it is in the recording
/// folder (or named like gsr names recordings), a replay otherwise.
pub fn file_saved(
    path: &Path,
    replay_dir: &Path,
    recording_dir: &Path,
    saved_at_ms: u64,
) -> Option<FileSaved> {
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    let kind = if path.starts_with(recording_dir) {
        FileKind::Recording
    } else if path.starts_with(replay_dir) || file_name.starts_with("Replay_") {
        FileKind::Replay
    } else if file_name.starts_with("Video_") {
        FileKind::Recording
    } else {
        FileKind::Replay
    };
    Some(FileSaved {
        kind,
        file_name,
        saved_at_ms,
    })
}

/// Why the process ended, for `error`.
fn exit_message(exit: std::process::ExitStatus, last_error: Option<String>) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(error) = last_error {
        return format!("GPU Screen Recorder stopped: {error}");
    }
    match (exit.code(), exit.signal()) {
        (Some(code), _) => format!("GPU Screen Recorder stopped unexpectedly (exit code {code})."),
        (None, Some(signal)) => {
            format!("GPU Screen Recorder stopped unexpectedly (killed by signal {signal}).")
        }
        _ => "GPU Screen Recorder stopped unexpectedly.".to_owned(),
    }
}

/// Registers the service as managed state and detects GPU Screen Recorder in the background.
/// Starts the replay too when `replay_on_start` is set (also when the window starts hidden).
/// Needs `settings::init` to have run.
pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let paths = app.path();
    let handle = app.clone();
    let config: Source<ReplayConfig> = Box::new(move || {
        let state = handle
            .try_state::<SettingsState>()
            .ok_or_else(|| "The settings are not available: restart the app.".to_owned())?;
        let recording = state.current().recording;
        // The settings only accept absolute folders, so a relative one is a default built on an
        // empty video folder: the system has no video folder and no home (see `settings::init`).
        if !recording.replay_dir.is_absolute() {
            return Err(NO_VIDEO_FOLDER.to_owned());
        }
        Ok(recording.replay_config(state.portal_token()))
    });
    // The runtime folder ($XDG_RUNTIME_DIR) is private to the user and cleared at logout; the
    // pid keeps two Redline processes (no single instance without a session bus) apart.
    let socket_name = format!("gsr-{}.sock", std::process::id());
    let socket = paths
        .runtime_dir()
        .map(|dir| dir.join("redline"))
        .or_else(|_| paths.app_cache_dir())
        .map(|dir| dir.join(socket_name))
        .map_err(|_| "No folder for GPU Screen Recorder's control socket.".to_owned());

    let service = Recorder::new(
        Environment {
            detect: Box::new(detect),
            config,
            socket,
        },
        AppEvents(app.clone()),
    );
    app.manage(service.clone());
    let handle_for_start = app.clone();
    let spawned = thread::Builder::new()
        .name("redline-recorder-detect".into())
        .spawn(move || {
            service.detect();
            // Read here rather than in `init`: by now `settings::init` has run whatever the
            // order in `setup`, and a missing settings state is said, not swallowed.
            let replay_on_start = match settings::recording_settings(&handle_for_start) {
                Some(recording) => recording.replay_on_start,
                None => {
                    eprintln!("redline: no settings, replay_on_start is ignored");
                    false
                }
            };
            if replay_on_start && let Err(error) = service.start() {
                eprintln!("redline: the replay did not start with the app: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("redline: GPU Screen Recorder detection did not start ({error})");
    }
}

/// Applies changed recording settings in the background: restarting a running replay waits for
/// the process to exit.
pub fn settings_changed<R: Runtime>(app: &AppHandle<R>) {
    let Some(service) = app
        .try_state::<Recorder>()
        .map(|service| service.inner().clone())
    else {
        return;
    };
    let spawned = thread::Builder::new()
        .name("redline-replay-reconfigure".into())
        .spawn(move || {
            if let Err(error) = service.reconfigure() {
                eprintln!("redline: the replay did not restart with the new settings: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("redline: the replay restart did not start ({error})");
    }
}

/// Stops the replay on exit (tray Quit, or closing the window without a tray).
pub fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    if let Some(service) = app.try_state::<Recorder>() {
        service.shutdown();
    }
}

/// Runs `job` on the blocking pool: every recorder action may wait on the process.
async fn blocking<T: Send + 'static>(
    service: &Recorder,
    job: impl FnOnce(Recorder) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let service = service.clone();
    tauri::async_runtime::spawn_blocking(move || job(service))
        .await
        .map_err(|error| format!("The recorder did not answer: {error}"))?
}

#[tauri::command]
pub async fn recorder_status(service: State<'_, Recorder>) -> Result<RecorderStatus, String> {
    blocking(&service, |service| Ok(service.status())).await
}

#[tauri::command]
pub async fn replay_start(service: State<'_, Recorder>) -> Result<RecorderStatus, String> {
    blocking(&service, |service| service.start()).await
}

#[tauri::command]
pub async fn replay_stop(service: State<'_, Recorder>) -> Result<RecorderStatus, String> {
    blocking(&service, |service| service.stop()).await
}

/// Starts a regular recording, or stops the running one (see [`Recorder::toggle_recording`]).
#[tauri::command]
pub async fn recording_toggle(service: State<'_, Recorder>) -> Result<RecorderStatus, String> {
    blocking(&service, |service| service.toggle_recording()).await
}

#[tauri::command]
pub async fn replay_save(service: State<'_, Recorder>) -> Result<(), String> {
    blocking(&service, |service| service.save()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::process::Command;
    use std::time::Instant;

    /// Collects what the service reports, in order.
    #[derive(Clone, Default)]
    struct Collector {
        states: Arc<Mutex<Vec<RecorderStatus>>>,
        files: Arc<Mutex<Vec<FileSaved>>>,
    }

    impl Events for Collector {
        fn state(&self, status: &RecorderStatus) {
            lock(&self.states).push(status.clone());
        }

        fn file_saved(&self, file: &FileSaved) {
            lock(&self.files).push(file.clone());
        }
    }

    impl Collector {
        fn saw(&self, check: impl Fn(&RecorderStatus) -> bool) -> bool {
            lock(&self.states).iter().any(check)
        }
    }

    const DEFAULT_REPLAY_SECONDS: u32 = 60;

    /// The default recording settings with the folders under `video_dir`.
    fn test_config(video_dir: &Path) -> ReplayConfig {
        settings::RecordingSettings::defaults(video_dir).replay_config(None)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("redline-replay-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("run")).unwrap();
        dir
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// A shell script standing in for gpu-screen-recorder. `$out` is `-o` (the replay folder, or
    /// the file of a recording without the replay), `$rec` is `-ro`; `body` runs after the
    /// arguments are read.
    fn fake_gsr(dir: &Path, body: &str) -> PathBuf {
        let script = dir.join("fake-gsr");
        let text = format!(
            "#!/bin/sh\n\
             [ \"$1\" = --version ] && {{ echo {}; exit 0; }}\n\
             while [ $# -gt 0 ]; do\n\
               [ \"$1\" = -o ] && out=$2; [ \"$1\" = -ro ] && rec=$2; shift\n\
             done\n\
             {body}\n",
            recorder::VERIFIED_VERSION
        );
        fs::write(&script, text).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        // A test thread that forks while the file is still open for writing makes exec fail
        // with ETXTBSY until that child execs: wait for the script to run once.
        wait_until("the fake script to be executable", || {
            match Command::new(&script).arg("--version").output() {
                Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => false,
                other => {
                    other.unwrap();
                    true
                }
            }
        });
        script
    }

    fn service(dir: &Path, body: &str) -> (Recorder, Collector) {
        let launcher = Launcher::Native(fake_gsr(dir, body));
        let video_dir = dir.to_owned();
        let events = Collector::default();
        let service = Recorder::new(
            Environment {
                detect: Box::new(move || Availability::Ready {
                    launcher: launcher.clone(),
                    warning: None,
                }),
                config: Box::new(move || Ok(test_config(&video_dir))),
                socket: Ok(dir.join("run").join("gsr.sock")),
            },
            events.clone(),
        );
        (service, events)
    }

    /// Plays gsr's IPC side: answers the readiness probe with an error, as gsr does for a name
    /// it does not know, `save-replay` and `stop-replay-recording` with the saved path, and
    /// `start-replay-recording` with a bare ok. Returns the request lines it received.
    fn serve_ipc(socket: &Path) -> Arc<Mutex<Vec<String>>> {
        let listener = UnixListener::bind(socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let reply = if line.contains("\"save-replay\"") {
                    "{\"id\":1,\"result\":\"ok\",\"data\":\"/v/Replay_1.mkv\"}\n"
                } else if line.contains("\"start-replay-recording\"") {
                    "{\"id\":1,\"result\":\"ok\"}\n"
                } else if line.contains("\"stop-replay-recording\"") {
                    "{\"id\":1,\"result\":\"ok\",\"data\":\"/v/Video_1.mkv\"}\n"
                } else {
                    "{\"id\":1,\"result\":\"error\",\"data\":\"unknown request name\"}\n"
                };
                lock(&seen).push(line);
                let _ = (&stream).write_all(reply.as_bytes());
            }
        });
        requests
    }

    fn names(requests: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        lock(requests)
            .iter()
            .filter_map(|line| {
                let request: serde_json::Value = serde_json::from_str(line).ok()?;
                Some(request["name"].as_str()?.to_owned())
            })
            .filter(|name| name != recorder::READY_PROBE)
            .collect()
    }

    #[test]
    fn a_recording_during_the_replay_uses_the_replay_process() {
        let dir = temp_dir("replay-recording");
        // SIGINT finishes the recording in progress and prints its path, as gsr does.
        let ready = dir.join("ready");
        let body = format!(
            "trap 'echo \"$rec/Video_kept.mkv\"; exit 0' INT\n: > '{}'\n{SLEEP}",
            ready.display()
        );
        let (service, events) = service(&dir, &body);
        let requests = serve_ipc(&dir.join("run").join("gsr.sock"));

        service.start().unwrap();
        wait_until("on", || service.status().replay == ReplayState::On);
        wait_until("the trap", || ready.exists());
        let started = service.toggle_recording().unwrap();
        assert!(started.recording);
        assert_eq!(started.replay, ReplayState::On);
        assert!(!service.toggle_recording().unwrap().recording);
        assert_eq!(
            names(&requests),
            ["start-replay-recording", "stop-replay-recording"]
        );

        // Turning the replay off while recording ends the recording and keeps the file.
        assert!(service.toggle_recording().unwrap().recording);
        let stopped = service.stop().unwrap();
        assert_eq!(
            (stopped.replay, stopped.recording),
            (ReplayState::Off, false)
        );
        wait_until("file-saved", || !lock(&events.files).is_empty());
        let file = lock(&events.files)[0].clone();
        assert_eq!(file.kind, FileKind::Recording);
        assert_eq!(file.file_name, "Video_kept.mkv");
        // Never a second process: the replay process did all of it.
        assert_eq!(names(&requests).len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recording_without_the_replay_is_one_process_that_keeps_its_file() {
        let dir = temp_dir("recording");
        // In recording mode `$out` is the file; SIGINT closes it and prints its path.
        let ready = dir.join("ready");
        let body = format!(
            "trap 'echo \"$out\"; exit 0' INT\nrm -f '{0}'\n: > '{0}'\n{SLEEP}",
            ready.display()
        );
        let (service, events) = service(&dir, &body);

        let started = service.toggle_recording().unwrap();
        // SIGINT before the script has set its trap would kill it without the file's path.
        wait_until("the trap", || ready.exists());
        assert_eq!(
            (started.replay, started.recording),
            (ReplayState::Off, true)
        );
        // The replay and the recording never run as two processes.
        assert_eq!(service.start().unwrap_err(), RECORDING_RUNS);
        assert!(service.stop().unwrap().recording, "replay_stop leaves it");
        assert_eq!(service.save().unwrap_err(), NOT_RUNNING);

        let stopped = service.toggle_recording().unwrap();
        assert_eq!(
            (stopped.replay, stopped.recording),
            (ReplayState::Off, false)
        );
        wait_until("file-saved", || !lock(&events.files).is_empty());
        let file = lock(&events.files)[0].clone();
        assert_eq!(file.kind, FileKind::Recording);
        assert!(file.file_name.starts_with("Video_") && file.file_name.ends_with(".mkv"));
        assert!(dir.join("Redline").join("Recordings").is_dir());

        // Quitting during a recording keeps the file too.
        let _ = fs::remove_file(&ready);
        assert!(service.toggle_recording().unwrap().recording);
        wait_until("the second trap", || ready.exists());
        service.shutdown();
        assert!(!service.status().recording);
        wait_until("the second file-saved", || lock(&events.files).len() == 2);
        // With nothing running the replay starts again.
        assert_eq!(service.start().unwrap().replay, ReplayState::Starting);
        service.shutdown();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recording_process_that_dies_ends_the_recording_with_the_reason() {
        let dir = temp_dir("recording-exit");
        let body = "echo 'gsr error: failed to get the portal session' >&2\nexit 3";
        let (service, _) = service(&dir, body);
        assert!(service.toggle_recording().unwrap().recording);
        wait_until("the end", || !service.status().recording);
        let status = service.status();
        assert_eq!(status.replay, ReplayState::Off);
        assert_eq!(
            status.error.as_deref(),
            Some("GPU Screen Recorder stopped: failed to get the portal session")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    const SLEEP: &str = "while :; do sleep 0.05; done";
    const LOOP: &str = "trap 'exit 0' INT\nwhile :; do sleep 0.05; done";

    #[test]
    fn a_replay_starts_saves_and_stops() {
        let dir = temp_dir("cycle");
        let body = format!(
            "echo \"$out/Replay_2026-10-29_14-04-22.mkv\"\n\
             echo \"gsr error: Failed to save replay\"\n{LOOP}"
        );
        let (service, events) = service(&dir, &body);
        serve_ipc(&dir.join("run").join("gsr.sock"));

        let started = service.start().unwrap();
        assert_eq!(started.replay, ReplayState::Starting);
        assert!(started.available);
        assert_eq!(started.replay_seconds, DEFAULT_REPLAY_SECONDS);
        wait_until("on", || service.status().replay == ReplayState::On);
        // A second start changes nothing: one session for the whole app.
        assert_eq!(service.start().unwrap().replay, ReplayState::On);

        wait_until("file-saved", || !lock(&events.files).is_empty());
        let file = lock(&events.files)[0].clone();
        assert_eq!(file.kind, FileKind::Replay);
        assert_eq!(file.file_name, "Replay_2026-10-29_14-04-22.mkv");
        assert!(file.saved_at_ms > 0);
        wait_until("the failed save", || service.status().error.is_some());
        assert_eq!(
            service.status().error.as_deref(),
            Some("GPU Screen Recorder: Failed to save replay")
        );

        service.save().unwrap();
        let stopped = service.stop().unwrap();
        assert_eq!(stopped.replay, ReplayState::Off);
        assert_eq!(stopped.error, None);
        assert_eq!(service.save().unwrap_err(), NOT_RUNNING);
        for state in [ReplayState::Starting, ReplayState::On, ReplayState::Off] {
            assert!(
                events.saw(|status| status.replay == state),
                "{state:?} not sent"
            );
        }
        assert!(!events.saw(|status| status.replay == ReplayState::Error));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_settings_change_restarts_a_running_replay_with_the_new_settings() {
        let dir = temp_dir("reconfigure");
        let launcher = Launcher::Native(fake_gsr(&dir, LOOP));
        let seconds = Arc::new(Mutex::new(60));
        let source = Arc::clone(&seconds);
        let video_dir = dir.clone();
        let events = Collector::default();
        let service = Recorder::new(
            Environment {
                detect: Box::new(move || Availability::Ready {
                    launcher: launcher.clone(),
                    warning: None,
                }),
                config: Box::new(move || {
                    let mut config = test_config(&video_dir);
                    config.replay_seconds = *lock(&source);
                    Ok(config)
                }),
                socket: Ok(dir.join("run").join("gsr.sock")),
            },
            events.clone(),
        );
        assert_eq!(
            service.launcher(),
            Ok(Launcher::Native(dir.join("fake-gsr")))
        );

        // Off: only the length shown for the next replay changes.
        *lock(&seconds) = 90;
        let status = service.reconfigure().unwrap();
        assert_eq!(
            (status.replay, status.replay_seconds),
            (ReplayState::Off, 90)
        );
        assert!(events.saw(|status| status.replay_seconds == 90));

        // Running: stopped and started again with the new settings.
        service.start().unwrap();
        *lock(&seconds) = 120;
        let status = service.reconfigure().unwrap();
        assert_eq!(
            (status.replay, status.replay_seconds),
            (ReplayState::Starting, 120)
        );
        let replays: Vec<_> = lock(&events.states).iter().map(|s| s.replay).collect();
        assert!(
            replays.ends_with(&[ReplayState::Off, ReplayState::Starting]),
            "{replays:?}"
        );
        service.shutdown();
        assert_eq!(service.status().replay, ReplayState::Off);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_before_capture_runs_is_refused() {
        let dir = temp_dir("starting");
        // No IPC server: gsr is still waiting for the screen.
        let (service, _) = service(&dir, LOOP);
        assert_eq!(service.start().unwrap().replay, ReplayState::Starting);
        assert_eq!(service.save().unwrap_err(), STILL_STARTING);
        assert_eq!(service.toggle_recording().unwrap_err(), STILL_STARTING);
        assert_eq!(service.stop().unwrap().replay, ReplayState::Off);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_exit_nobody_asked_for_is_an_error_with_the_reason() {
        let dir = temp_dir("exit");
        let body = "echo 'gsr error: failed to get the portal session' >&2\nexit 3";
        let (service, events) = service(&dir, body);
        service.start().unwrap();
        wait_until("error", || service.status().replay == ReplayState::Error);
        assert_eq!(
            service.status().error.as_deref(),
            Some("GPU Screen Recorder stopped: failed to get the portal session")
        );
        assert!(events.saw(|status| status.replay == ReplayState::Error));
        // Stopping acknowledges the error.
        let stopped = service.stop().unwrap();
        assert_eq!((stopped.replay, stopped.error), (ReplayState::Off, None));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_process_killed_from_outside_is_an_error() {
        let dir = temp_dir("killed");
        let body = format!("echo $$ > \"$out/../pid\"\n{LOOP}");
        let (service, _) = service(&dir, &body);
        service.start().unwrap();
        let pid_file = dir.join("Redline").join("pid");
        wait_until("the pid", || {
            fs::read_to_string(&pid_file).is_ok_and(|pid| pid.ends_with('\n'))
        });
        let pid = fs::read_to_string(&pid_file).unwrap();
        assert!(
            Command::new("kill")
                .args(["-KILL", pid.trim()])
                .status()
                .unwrap()
                .success()
        );
        wait_until("error", || service.status().replay == ReplayState::Error);
        assert_eq!(
            service.status().error.as_deref(),
            Some("GPU Screen Recorder stopped unexpectedly (killed by signal 9).")
        );
        // The next start works again.
        assert_eq!(service.start().unwrap().replay, ReplayState::Starting);
        service.shutdown();
        assert_eq!(service.status().replay, ReplayState::Off);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_running_replay_stops_even_if_gpu_screen_recorder_disappeared() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = temp_dir("vanished");
        let launcher = Launcher::Native(fake_gsr(&dir, LOOP));
        let installed = Arc::new(AtomicBool::new(true));
        let seen = Arc::clone(&installed);
        let video_dir = dir.clone();
        let service = Recorder::new(
            Environment {
                detect: Box::new(move || {
                    if seen.load(Ordering::SeqCst) {
                        Availability::Ready {
                            launcher: launcher.clone(),
                            warning: None,
                        }
                    } else {
                        Availability::Missing(MISSING.to_owned())
                    }
                }),
                config: Box::new(move || Ok(test_config(&video_dir))),
                socket: Ok(dir.join("run").join("gsr.sock")),
            },
            Collector::default(),
        );
        service.start().unwrap();
        // Uninstalled (or updated away) while the replay runs.
        installed.store(false, Ordering::SeqCst);
        service.detect();
        assert!(!service.status().available);
        assert_eq!(service.stop().unwrap().replay, ReplayState::Off);
        // With nothing left to stop, the missing tool is the answer again.
        assert_eq!(service.stop().unwrap_err(), MISSING);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_gpu_screen_recorder_every_command_says_what_to_install() {
        let events = Collector::default();
        let service = Recorder::new(
            Environment {
                detect: Box::new(|| Availability::Missing(MISSING.to_owned())),
                config: Box::new(|| Ok(test_config(Path::new("/videos")))),
                socket: Ok(PathBuf::from("/nowhere/gsr.sock")),
            },
            events.clone(),
        );
        let status = service.status();
        assert!(!status.available);
        assert_eq!(status.unavailable_reason.as_deref(), Some(MISSING));
        assert_eq!(status.replay, ReplayState::Off);
        assert_eq!(service.start().unwrap_err(), MISSING);
        assert_eq!(service.save().unwrap_err(), MISSING);
        assert_eq!(service.stop().unwrap_err(), MISSING);
        assert_eq!(service.toggle_recording().unwrap_err(), MISSING);
        assert_eq!(service.launcher().unwrap_err(), MISSING);
        assert!(events.saw(|status| status.unavailable_reason.is_some()));
        // The status still says off: a missing tool is not a replay error.
        assert_eq!(service.status().replay, ReplayState::Off);
    }

    #[test]
    fn an_unverified_version_is_a_warning_not_a_block() {
        let service = Recorder::new(
            Environment {
                detect: Box::new(|| Availability::Ready {
                    launcher: Launcher::Flatpak,
                    warning: recorder::version_notice(Some("6.2.0")),
                }),
                config: Box::new(|| Err("No video folder to save replays in.".to_owned())),
                socket: Ok(PathBuf::from("/nowhere/gsr.sock")),
            },
            Collector::default(),
        );
        let status = service.status();
        assert!(status.available);
        assert!(status.warning.unwrap().contains("6.2.0"));
        // A start that cannot happen reports why.
        assert_eq!(
            service.start().unwrap_err(),
            "No video folder to save replays in."
        );
        assert_eq!(service.status().replay, ReplayState::Error);
    }

    #[test]
    fn the_payloads_use_the_documented_shape() {
        let status = RecorderStatus {
            available: true,
            unavailable_reason: None,
            replay: ReplayState::Starting,
            recording: false,
            replay_seconds: 60,
            error: None,
            warning: None,
        };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({
                "available": true,
                "unavailable_reason": null,
                "replay": "starting",
                "recording": false,
                "replay_seconds": 60,
                "error": null,
                "warning": null,
            })
        );
        for (state, text) in [
            (ReplayState::Off, "off"),
            (ReplayState::On, "on"),
            (ReplayState::Error, "error"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), text);
        }
        let file = FileSaved {
            kind: FileKind::Recording,
            file_name: "Video_1.mkv".into(),
            saved_at_ms: 5,
        };
        assert_eq!(
            serde_json::to_value(&file).unwrap(),
            serde_json::json!({ "kind": "recording", "file_name": "Video_1.mkv", "saved_at_ms": 5 })
        );
    }

    #[test]
    fn saved_files_are_named_and_sorted_by_folder() {
        let replays = Path::new("/v/Redline/Replays");
        let recordings = Path::new("/v/Redline/Recordings");
        let saved = |path: &str| file_saved(Path::new(path), replays, recordings, 1).unwrap();

        let replay = saved("/v/Redline/Replays/Replay_2026-10-29_14-04-22.mkv");
        assert_eq!(replay.kind, FileKind::Replay);
        assert_eq!(replay.file_name, "Replay_2026-10-29_14-04-22.mkv");
        let recording = saved("/v/Redline/Recordings/2026-10-29/Video_2026-10-29_14-05-00.mkv");
        assert_eq!(recording.kind, FileKind::Recording);
        assert_eq!(recording.file_name, "Video_2026-10-29_14-05-00.mkv");
        // Elsewhere, gsr's own names decide.
        assert_eq!(saved("/elsewhere/Video_1.mkv").kind, FileKind::Recording);
        assert_eq!(saved("/elsewhere/Replay_1.mkv").kind, FileKind::Replay);
        assert!(file_saved(Path::new("/"), replays, recordings, 1).is_none());
    }

    #[test]
    fn exit_messages_name_the_cause() {
        use std::os::unix::process::ExitStatusExt;
        let code = std::process::ExitStatus::from_raw(3 << 8);
        assert_eq!(
            exit_message(code, None),
            "GPU Screen Recorder stopped unexpectedly (exit code 3)."
        );
        assert_eq!(
            exit_message(code, Some("no portal".into())),
            "GPU Screen Recorder stopped: no portal"
        );
    }
}
