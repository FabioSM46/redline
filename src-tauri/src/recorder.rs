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
//! Extension points for the issues built on this one:
//! - settings (#29): [`Environment::config`] is the single source of the [`ReplayConfig`];
//!   [`default_config`] holds today's fixed defaults.
//! - recording (#30): [`RecorderStatus::recording`] and [`FileKind::Recording`] exist already;
//!   requests to the running process go through its IPC socket ([`Running::socket`]), stopping
//!   through [`Recorder::stop`].
//! - gallery and notifications (#31): subscribe to [`Events::file_saved`] (the folders are in the
//!   config).
//! - command-line actions (#32): call [`Recorder::start`], [`Recorder::stop`], [`Recorder::save`]
//!   from the single-instance callback, as the commands do.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redline_core::recorder::{
    self, AudioTrack, Launcher, Output, ReadyProbe, ReplayConfig, ReplaySession,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

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

/// Defaults used until the recording settings exist (#29).
const DEFAULT_REPLAY_SECONDS: u32 = 60;
const DEFAULT_FPS: u32 = 60;
const VIDEO_SUBFOLDER: &str = "Redline";
const REPLAY_FOLDER: &str = "Replays";
const RECORDING_FOLDER: &str = "Recordings";

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
    /// A regular recording is running (#30).
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

/// The replay settings until the recording settings land (#29): portal capture, 60 s, 60 fps,
/// mkv, desktop audio and microphone on two tracks, folders under the user's video folder.
pub fn default_config(video_dir: &Path) -> ReplayConfig {
    let base = video_dir.join(VIDEO_SUBFOLDER);
    ReplayConfig {
        capture: "portal".into(),
        fps: DEFAULT_FPS,
        replay_seconds: DEFAULT_REPLAY_SECONDS,
        container: "mkv".into(),
        codec: None,
        replay_dir: base.join(REPLAY_FOLDER),
        recording_dir: base.join(RECORDING_FOLDER),
        audio: vec![
            AudioTrack::single("default_output"),
            AudioTrack::single("default_input"),
        ],
    }
}

type Source<T> = Box<dyn Fn() -> Result<T, String> + Send + Sync>;

/// What the service needs from its surroundings; tests pass a fake process.
pub struct Environment {
    pub detect: Box<dyn Fn() -> Availability + Send + Sync>,
    /// The settings of the next replay (#29 replaces this source).
    pub config: Source<ReplayConfig>,
    /// Where the session's IPC socket goes; private to this app instance.
    pub socket: Result<PathBuf, String>,
}

/// The running session.
struct Running {
    session: ReplaySession,
    socket: PathBuf,
}

struct ServiceState {
    availability: Option<Availability>,
    status: RecorderStatus,
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
        if self.state().running.is_some() {
            return Ok(self.state().status.clone());
        }
        // Detected again when missing: installing it does not require a restart.
        // Bound first: a guard in the match scrutinee would live through the arms.
        let known = self.state().availability.clone();
        let launcher = match known {
            Some(Availability::Ready { launcher, .. }) => launcher,
            _ => match self.refresh_availability() {
                Availability::Ready { launcher, .. } => launcher,
                Availability::Missing(reason) => return Err(reason),
            },
        };
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
        self.ensure_available()?;
        let mut state = self.state();
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
                (Some(_), _) => return Err(STILL_STARTING.to_owned()),
                (None, _) => return Err(NOT_RUNNING.to_owned()),
            }
        };
        recorder::save_replay(&socket, SAVE_TIMEOUT)
            .map(|_| ())
            .map_err(|error| format!("Could not save the replay: {error:#}"))
    }

    /// Stops the replay when the app exits.
    pub fn shutdown(&self) {
        if self.state().running.is_some() {
            let _ = self.stop();
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
            .name("redline-replay-supervisor".into())
            .spawn(move || {
                let mut probe = Some(ReadyProbe::new(&socket));
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
                            eprintln!("redline: cannot read the replay process state ({error})");
                            None
                        }
                    };
                    if let Some(exit) = exited {
                        let message = exit_message(exit, running.session.last_error());
                        state.running = None;
                        state.status.replay = ReplayState::Error;
                        state.status.recording = false;
                        state.status.error = Some(message);
                    } else if ready && state.status.replay == ReplayState::Starting {
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
pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let paths = app.path();
    let video_dir = paths
        .video_dir()
        .or_else(|_| paths.home_dir().map(|home| home.join("Videos")));
    let config: Source<ReplayConfig> = Box::new(move || match &video_dir {
        Ok(dir) => Ok(default_config(dir)),
        Err(_) => Err("No video folder to save replays in.".to_owned()),
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
    let spawned = thread::Builder::new()
        .name("redline-recorder-detect".into())
        .spawn(move || service.detect());
    if let Err(error) = spawned {
        eprintln!("redline: GPU Screen Recorder detection did not start ({error})");
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

    /// A shell script standing in for gpu-screen-recorder. `$out` is the replay folder (`-o`);
    /// `body` runs after the arguments are read.
    fn fake_gsr(dir: &Path, body: &str) -> PathBuf {
        let script = dir.join("fake-gsr");
        let text = format!(
            "#!/bin/sh\n\
             [ \"$1\" = --version ] && {{ echo {}; exit 0; }}\n\
             while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\n\
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
                config: Box::new(move || Ok(default_config(&video_dir))),
                socket: Ok(dir.join("run").join("gsr.sock")),
            },
            events.clone(),
        );
        (service, events)
    }

    /// Plays gsr's IPC side: answers the readiness probe with an error, as gsr does for a name
    /// it does not know, and `save-replay` with the saved path.
    fn serve_ipc(socket: &Path) {
        let listener = UnixListener::bind(socket).unwrap();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let reply = if line.contains("\"save-replay\"") {
                    "{\"id\":1,\"result\":\"ok\",\"data\":\"/v/Replay_1.mkv\"}\n"
                } else {
                    "{\"id\":1,\"result\":\"error\",\"data\":\"unknown request name\"}\n"
                };
                let _ = (&stream).write_all(reply.as_bytes());
            }
        });
    }

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
    fn saving_before_capture_runs_is_refused() {
        let dir = temp_dir("starting");
        // No IPC server: gsr is still waiting for the screen.
        let (service, _) = service(&dir, LOOP);
        assert_eq!(service.start().unwrap().replay, ReplayState::Starting);
        assert_eq!(service.save().unwrap_err(), STILL_STARTING);
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
        let pid_file = dir.join(VIDEO_SUBFOLDER).join("pid");
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
    fn without_gpu_screen_recorder_every_command_says_what_to_install() {
        let events = Collector::default();
        let service = Recorder::new(
            Environment {
                detect: Box::new(|| Availability::Missing(MISSING.to_owned())),
                config: Box::new(|| Ok(default_config(Path::new("/videos")))),
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
    fn the_defaults_live_under_the_video_folder() {
        let config = default_config(Path::new("/videos"));
        assert_eq!(config.capture, "portal");
        assert_eq!((config.replay_seconds, config.fps), (60, 60));
        assert_eq!(config.container, "mkv");
        assert_eq!(config.replay_dir, Path::new("/videos/Redline/Replays"));
        assert_eq!(
            config.recording_dir,
            Path::new("/videos/Redline/Recordings")
        );
        let tracks: Vec<_> = config.audio.iter().map(|track| track.0.clone()).collect();
        assert_eq!(tracks, [vec!["default_output"], vec!["default_input"]]);
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
