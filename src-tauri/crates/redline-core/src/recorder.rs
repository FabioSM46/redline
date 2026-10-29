//! Control of GPU Screen Recorder (gsr) as an external process.
//!
//! Verified against GPU Screen Recorder [`VERIFIED_VERSION`] (its man page and source): options,
//! signals and the IPC protocol are not stable promises, so a different version is reported
//! (see [`version_notice`]), never blocked.
//!
//! A session talks only to the process it started, through two channels:
//! - **IPC** (`-ipc <socket>`): newline-terminated JSON requests, each one replied to. gsr binds
//!   the socket at startup but serves requests only once capture has started, so the first reply
//!   (to [`READY_PROBE`]) marks the replay as running ([`ReadyProbe`]). Saving goes through it,
//!   and the reply arrives when the file has been written.
//! - **Signals**, only to stop: SIGINT to the gsr process among the descendants of our own child,
//!   then SIGKILL to that whole tree if it does not exit. With the native binary that is the child
//!   itself. With the Flatpak, `flatpak run` becomes `bwrap`, which runs gsr inside the sandbox
//!   and does not forward signals: gsr is found by walking `/proc` down from our child, never by
//!   name across the user's processes.
//!
//! gsr writes the path of every saved file to stdout, and a `gsr error: ...` line when a save
//! fails ([`parse_output_line`]).
//!
//! We use the native binary when it is on PATH, otherwise the Flatpak.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// The GPU Screen Recorder version whose options, signals and IPC protocol this module was
/// checked against.
pub const VERIFIED_VERSION: &str = "6.1.6";
pub const FLATPAK_ID: &str = "com.dec05eba.gpu_screen_recorder";
const NATIVE_BINARY: &str = "gpu-screen-recorder";
/// The process `comm` in /proc, truncated by the kernel to 15 characters.
const GSR_COMM: &str = "gpu-screen-reco";
/// A request name gsr does not know: it answers with an error, and the answer is all we need.
pub const READY_PROBE: &str = "redline-ready-probe";
/// How long gsr gets to exit after SIGINT before the whole tree is killed.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const KILL_TIMEOUT: Duration = Duration::from_secs(2);
const WAIT_STEP: Duration = Duration::from_millis(20);
/// Timeout of requests that gsr answers at once (everything but saving).
pub const IPC_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher {
    /// A gsr binary run directly: `gpu-screen-recorder` from PATH, or an explicit path.
    Native(PathBuf),
    Flatpak,
}

impl Launcher {
    pub fn detect() -> Result<Launcher> {
        let native = Launcher::Native(PathBuf::from(NATIVE_BINARY));
        let runs = native
            .command(None)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();
        if runs {
            return Ok(native);
        }
        let flatpak = Command::new("flatpak")
            .args(["info", FLATPAK_ID])
            .stdin(Stdio::null())
            .output();
        if matches!(flatpak, Ok(out) if out.status.success()) {
            return Ok(Launcher::Flatpak);
        }
        bail!("GPU Screen Recorder not found (neither on PATH nor as Flatpak {FLATPAK_ID})")
    }

    /// The version gsr reports with `--version`, for example `6.1.6`.
    pub fn version(&self) -> Result<String> {
        let out = self
            .command(None)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .context("running gpu-screen-recorder --version failed")?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("gpu-screen-recorder --version printed nothing"))
    }

    /// `share`: a host folder the Flatpak sandbox must see (the IPC socket's folder); the native
    /// binary sees everything already.
    fn command(&self, share: Option<&Path>) -> Command {
        match self {
            Launcher::Native(program) => Command::new(program),
            Launcher::Flatpak => {
                let mut cmd = Command::new("flatpak");
                cmd.args(["run", "--command=gpu-screen-recorder"]);
                if let Some(dir) = share {
                    let mut option = std::ffi::OsString::from("--filesystem=");
                    option.push(dir);
                    cmd.arg(option);
                }
                cmd.arg(FLATPAK_ID);
                cmd
            }
        }
    }
}

/// `None` when `version` is the verified one, otherwise a sentence for the user: a different or
/// unreadable version is a warning, not a block.
pub fn version_notice(version: Option<&str>) -> Option<String> {
    match version {
        Some(VERIFIED_VERSION) => None,
        Some(version) => Some(format!(
            "GPU Screen Recorder {version} is installed, but Redline is verified with \
             {VERIFIED_VERSION}: some features may not work."
        )),
        None => Some(format!(
            "Could not read the GPU Screen Recorder version; Redline is verified with \
             {VERIFIED_VERSION}."
        )),
    }
}

/// One audio track of the output file. Several sources in the same track are mixed.
/// Valid sources: `default_output`, `default_input`, `app:<name>`, a device name.
#[derive(Debug, Clone)]
pub struct AudioTrack(pub Vec<String>);

impl AudioTrack {
    pub fn single(source: impl Into<String>) -> Self {
        Self(vec![source.into()])
    }
}

#[derive(Debug, Clone)]
pub struct ReplayConfig {
    /// `portal` (Wayland, asks for the screen once), `screen`, or a monitor name
    pub capture: String,
    pub fps: u32,
    pub replay_seconds: u32,
    /// mkv, to carry several audio tracks without surprises
    pub container: String,
    /// h264, hevc, av1... `None` = gsr's choice
    pub codec: Option<String>,
    pub replay_dir: PathBuf,
    /// Where regular recordings started during the replay go
    pub recording_dir: PathBuf,
    pub audio: Vec<AudioTrack>,
}

impl ReplayConfig {
    fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-w".into(),
            self.capture.clone(),
            "-f".into(),
            self.fps.to_string(),
            "-r".into(),
            self.replay_seconds.to_string(),
            "-c".into(),
            self.container.clone(),
            "-o".into(),
            self.replay_dir.display().to_string(),
            "-ro".into(),
            self.recording_dir.display().to_string(),
        ];
        if self.capture == "portal" {
            args.extend(["-restore-portal-session".into(), "yes".into()]);
        }
        if let Some(codec) = &self.codec {
            args.extend(["-k".into(), codec.clone()]);
        }
        for track in &self.audio {
            args.extend(["-a".into(), track.0.join("|")]);
        }
        args
    }
}

/// One meaningful line of gsr's stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// The absolute path of a file gsr has just saved (replay or recording).
    Saved(PathBuf),
    /// A `gsr error: ...` line: a save or a recording failed; the replay keeps running.
    Failed(String),
}

/// Reads one stdout line: a saved file's path, a failure, or nothing worth reporting.
pub fn parse_output_line(line: &str) -> Option<Output> {
    if let Some(message) = line.trim().strip_prefix("gsr error:") {
        return Some(Output::Failed(message.trim().to_owned()));
    }
    line.starts_with('/')
        .then(|| Output::Saved(PathBuf::from(line)))
}

/// gsr in replay mode, started and owned by us.
pub struct ReplaySession {
    child: Child,
    launcher: Launcher,
    socket: PathBuf,
    /// The last `gsr error: ...` line of stderr, to explain an exit.
    last_error: Arc<Mutex<Option<String>>>,
}

impl ReplaySession {
    /// Starts gsr with its IPC socket at `socket`. `on_output` runs on a reader thread for every
    /// [`Output`] line, until gsr closes stdout. gsr's stderr is forwarded to ours.
    pub fn start(
        launcher: &Launcher,
        config: &ReplayConfig,
        socket: &Path,
        on_output: impl FnMut(Output) + Send + 'static,
    ) -> Result<ReplaySession> {
        for dir in [&config.replay_dir, &config.recording_dir] {
            fs::create_dir_all(dir)
                .with_context(|| format!("could not create the folder {}", dir.display()))?;
        }
        let socket_dir = socket
            .parent()
            .context("the IPC socket path has no folder")?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(socket_dir)
            .context("could not create the IPC socket folder")?;

        let mut child = command_line(launcher, config, socket)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("starting gpu-screen-recorder failed")?;

        let stdout = child.stdout.take().context("gsr stdout is not piped")?;
        let stderr = child.stderr.take().context("gsr stderr is not piped")?;
        let mut on_output = on_output;
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(output) = parse_output_line(&line) {
                    on_output(output);
                }
            }
        });
        let last_error = Arc::new(Mutex::new(None));
        let last = Arc::clone(&last_error);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                // Forwarded as `Stdio::inherit` would, so gsr's own log stays visible.
                eprintln!("{line}");
                if let Some(message) = line.strip_prefix("gsr error:") {
                    *last.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(message.trim().to_owned());
                }
            }
        });

        Ok(ReplaySession {
            child,
            launcher: launcher.clone(),
            socket: socket.to_owned(),
            last_error,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Watches for the moment this session starts serving IPC requests (capture running).
    pub fn ready_probe(&self) -> ReadyProbe {
        ReadyProbe::new(&self.socket)
    }

    /// Saves the replay and returns the saved file's path, once gsr has written it.
    pub fn save_replay(&self, timeout: Duration) -> Result<PathBuf> {
        save_replay(&self.socket, timeout)
    }

    /// Starts or stops a regular recording into `recording_dir` while the replay runs.
    pub fn toggle_recording(&self) -> Result<()> {
        ipc_request(&self.socket, "toggle-replay-recording", None, IPC_TIMEOUT).map(|_| ())
    }

    /// The exit status if gsr has exited, without blocking.
    pub fn try_exit(&mut self) -> Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.remove_socket();
        }
        Ok(status)
    }

    /// The last error gsr logged, to explain why it exited.
    pub fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// SIGINT to our gsr, then SIGKILL to our whole process tree after [`STOP_TIMEOUT`].
    /// In replay mode stopping saves nothing.
    pub fn stop(&mut self) -> Result<ExitStatus> {
        if let Some(status) = self.try_exit()? {
            return Ok(status);
        }
        for pid in self.gsr_pids() {
            // A process that has just exited is not an error: the wait below decides.
            let _ = send_signal(pid, libc::SIGINT);
        }
        if let Some(status) = self.wait_for(STOP_TIMEOUT)? {
            return Ok(status);
        }
        let mut tree = vec![self.child.id()];
        tree.extend(
            descendants(Path::new("/proc"), self.child.id())
                .into_iter()
                .map(|(pid, _)| pid),
        );
        for pid in tree {
            let _ = send_signal(pid, libc::SIGKILL);
        }
        match self.wait_for(KILL_TIMEOUT)? {
            Some(status) => Ok(status),
            None => bail!("gpu-screen-recorder did not exit after SIGKILL"),
        }
    }

    fn wait_for(&mut self, timeout: Duration) -> Result<Option<ExitStatus>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_exit()? {
                return Ok(Some(status));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(WAIT_STEP);
        }
    }

    /// The gsr processes this session started. Called only while the child is not reaped, so
    /// its pid cannot have been reused.
    fn gsr_pids(&self) -> Vec<u32> {
        match self.launcher {
            Launcher::Native(_) => vec![self.child.id()],
            Launcher::Flatpak => descendants(Path::new("/proc"), self.child.id())
                .into_iter()
                .filter(|(_, comm)| comm == GSR_COMM)
                .map(|(pid, _)| pid)
                .collect(),
        }
    }

    /// gsr removes its socket on a clean exit; after a crash or SIGKILL the file stays.
    fn remove_socket(&self) {
        if fs::symlink_metadata(&self.socket).is_ok_and(|meta| meta.file_type().is_socket()) {
            let _ = fs::remove_file(&self.socket);
        }
    }
}

impl Drop for ReplaySession {
    /// A session is never left running behind the owner's back.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.stop();
        }
    }
}

/// The full command for a replay session with its IPC socket at `socket`.
fn command_line(launcher: &Launcher, config: &ReplayConfig, socket: &Path) -> Command {
    let mut cmd = launcher.command(socket.parent());
    cmd.args(config.args()).arg("-ipc").arg(socket);
    cmd
}

fn send_signal(pid: u32, signal: libc::c_int) -> io::Result<()> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| io::Error::from(ErrorKind::InvalidInput))?;
    // SAFETY: kill() touches no memory, at worst it fails with errno
    if unsafe { libc::kill(pid, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Every descendant of `root` under `proc_root` (normally `/proc`), parents before children,
/// with its `comm`.
fn descendants(proc_root: &Path, root: u32) -> Vec<(u32, String)> {
    let mut children: HashMap<u32, Vec<(u32, String)>> = HashMap::new();
    let Ok(entries) = fs::read_dir(proc_root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        if let Some((ppid, comm)) = parse_stat(&stat) {
            children.entry(ppid).or_default().push((pid, comm));
        }
    }
    let mut found = Vec::new();
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        for (pid, comm) in children.remove(&parent).unwrap_or_default() {
            queue.push(pid);
            found.push((pid, comm));
        }
    }
    found
}

/// `(ppid, comm)` from `/proc/<pid>/stat`. `comm` sits between the first `(` and the last `)`
/// and may itself contain spaces and parentheses.
fn parse_stat(stat: &str) -> Option<(u32, String)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat.get(open + 1..close)?.to_owned();
    let mut fields = stat.get(close + 1..)?.split_whitespace();
    let _state = fields.next()?;
    let ppid = fields.next()?.parse().ok()?;
    Some((ppid, comm))
}

/// Saves the replay of the session listening on `socket`; returns the saved file's path.
pub fn save_replay(socket: &Path, timeout: Duration) -> Result<PathBuf> {
    match ipc_request(socket, "save-replay", None, timeout)? {
        Some(Value::String(path)) => Ok(PathBuf::from(path)),
        _ => bail!("GPU Screen Recorder did not say where it saved the replay"),
    }
}

/// One request on gsr's IPC socket. `Ok(data)` for an `ok` reply; an `error` reply is an error
/// carrying gsr's message.
pub fn ipc_request(
    socket: &Path,
    name: &str,
    data: Option<Value>,
    timeout: Duration,
) -> Result<Option<Value>> {
    let mut stream = UnixStream::connect(socket).context("could not reach GPU Screen Recorder")?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(request_line(1, name, data).as_bytes())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("GPU Screen Recorder did not answer")?;
    parse_reply(&line)
}

fn request_line(id: u64, name: &str, data: Option<Value>) -> String {
    let mut request = json!({ "id": id, "name": name });
    if let Some(data) = data {
        request["data"] = data;
    }
    format!("{request}\n")
}

fn parse_reply(line: &str) -> Result<Option<Value>> {
    let reply: Value =
        serde_json::from_str(line.trim()).context("GPU Screen Recorder sent an invalid reply")?;
    let data = reply.get("data").filter(|d| !d.is_null()).cloned();
    match reply.get("result").and_then(Value::as_str) {
        Some("ok") => Ok(data),
        Some("error") => bail!(
            "{}",
            data.as_ref()
                .and_then(Value::as_str)
                .unwrap_or("GPU Screen Recorder refused the request")
        ),
        _ => bail!("GPU Screen Recorder sent an invalid reply"),
    }
}

/// Tells when gsr starts serving IPC requests, which it does only once capture has started (with
/// the portal, after the user has picked a screen). Never blocks.
pub struct ReadyProbe {
    socket: PathBuf,
    connection: Option<UnixStream>,
}

impl ReadyProbe {
    pub fn new(socket: &Path) -> Self {
        Self {
            socket: socket.to_owned(),
            connection: None,
        }
    }

    /// True once gsr has answered [`READY_PROBE`]. A missing socket or a closed connection only
    /// means "not yet": the next call tries again.
    pub fn poll(&mut self) -> bool {
        if self.connection.is_none() {
            self.connection = self.connect().ok();
        }
        let Some(connection) = self.connection.as_mut() else {
            return false;
        };
        let mut buffer = [0u8; 256];
        match connection.read(&mut buffer) {
            Ok(0) => {
                self.connection = None;
                false
            }
            Ok(_) => true,
            Err(error) if error.kind() == ErrorKind::WouldBlock => false,
            Err(_) => {
                self.connection = None;
                false
            }
        }
    }

    fn connect(&self) -> io::Result<UnixStream> {
        let mut stream = UnixStream::connect(&self.socket)?;
        // gsr queues the connection until it serves requests: the write lands in the buffer.
        stream.set_write_timeout(Some(IPC_TIMEOUT))?;
        stream.write_all(request_line(1, READY_PROBE, None).as_bytes())?;
        stream.set_nonblocking(true)?;
        Ok(stream)
    }
}

/// Audio source listed by gsr: (name to pass to `-a`, description).
pub fn list_audio_devices(launcher: &Launcher) -> Result<Vec<(String, String)>> {
    let out = launcher
        .command(None)
        .arg("--list-audio-devices")
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (name, desc) = l.split_once('|')?;
            Some((name.to_owned(), desc.to_owned()))
        })
        .collect())
}

/// Applications currently playing audio, usable as `app:<name>`.
pub fn list_application_audio(launcher: &Launcher) -> Result<Vec<String>> {
    let out = launcher
        .command(None)
        .arg("--list-application-audio")
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn config(capture: &str) -> ReplayConfig {
        ReplayConfig {
            capture: capture.into(),
            fps: 60,
            replay_seconds: 30,
            container: "mkv".into(),
            codec: None,
            replay_dir: PathBuf::from("/tmp/replay"),
            recording_dir: PathBuf::from("/tmp/rec"),
            audio: vec![
                AudioTrack::single("default_output"),
                AudioTrack(vec!["default_input".into(), "app:Discord".into()]),
            ],
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("redline-recorder-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn one_audio_flag_per_track_and_mixed_sources_joined() {
        let args = config("screen").args();
        let audio: Vec<_> = args
            .windows(2)
            .filter(|w| w[0] == "-a")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(audio, ["default_output", "default_input|app:Discord"]);
        assert!(!args.contains(&"-restore-portal-session".to_string()));
    }

    #[test]
    fn portal_capture_restores_the_session() {
        let args = config("portal").args();
        let i = args
            .iter()
            .position(|a| a == "-restore-portal-session")
            .unwrap();
        assert_eq!(args[i + 1], "yes");
    }

    #[test]
    fn the_native_command_passes_the_ipc_socket() {
        let socket = Path::new("/run/user/1000/redline/gsr-1.sock");
        let cmd = command_line(&Launcher::Native("gsr".into()), &config("portal"), socket);
        assert_eq!(cmd.get_program(), "gsr");
        let args = args_of(&cmd);
        assert_eq!(
            args[args.len() - 2..],
            ["-ipc", "/run/user/1000/redline/gsr-1.sock"]
        );
        assert!(!args.iter().any(|a| a.starts_with("--filesystem")));
    }

    #[test]
    fn the_flatpak_sees_the_socket_folder_and_options_come_before_the_app_id() {
        let socket = Path::new("/run/user/1000/redline/gsr-1.sock");
        let cmd = command_line(&Launcher::Flatpak, &config("portal"), socket);
        assert_eq!(cmd.get_program(), "flatpak");
        let args = args_of(&cmd);
        assert_eq!(
            args[..4],
            [
                "run",
                "--command=gpu-screen-recorder",
                "--filesystem=/run/user/1000/redline",
                FLATPAK_ID
            ]
        );
        assert_eq!(
            args[args.len() - 2..],
            ["-ipc", "/run/user/1000/redline/gsr-1.sock"]
        );
    }

    #[test]
    fn stdout_lines_are_saved_paths_failures_or_nothing() {
        assert_eq!(
            parse_output_line("/videos/Replay_2026-10-29_14-04-22.mkv"),
            Some(Output::Saved(
                "/videos/Replay_2026-10-29_14-04-22.mkv".into()
            ))
        );
        assert_eq!(
            parse_output_line("gsr error: Failed to save replay"),
            Some(Output::Failed("Failed to save replay".into()))
        );
        assert_eq!(parse_output_line(""), None);
        assert_eq!(parse_output_line("relative/file.mkv"), None);
        assert_eq!(parse_output_line("gsr info: update fps: 60"), None);
    }

    #[test]
    fn only_the_verified_version_has_no_notice() {
        assert_eq!(version_notice(Some(VERIFIED_VERSION)), None);
        let other = version_notice(Some("6.2.0")).unwrap();
        assert!(other.contains("6.2.0") && other.contains(VERIFIED_VERSION));
        assert!(version_notice(None).unwrap().contains(VERIFIED_VERSION));
    }

    #[test]
    fn stat_lines_with_odd_command_names_parse() {
        assert_eq!(
            parse_stat("42 (gpu-screen-reco) S 41 42 42 0 -1"),
            Some((41, "gpu-screen-reco".into()))
        );
        assert_eq!(
            parse_stat("7 (a) b (c)) R 3 7 7 0"),
            Some((3, "a) b (c)".into()))
        );
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn descendants_follow_only_our_tree() {
        let proc_root = temp_dir("proc");
        let fake = |pid: u32, comm: &str, ppid: u32| {
            let dir = proc_root.join(pid.to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("stat"), format!("{pid} ({comm}) S {ppid} 0 0 0")).unwrap();
        };
        // Our `flatpak run` became bwrap (100) → bwrap (101) → gsr (102).
        fake(100, "bwrap", 1);
        fake(101, "bwrap", 100);
        fake(102, GSR_COMM, 101);
        // Another gsr of the same user, not started by us.
        fake(200, GSR_COMM, 1);
        fs::create_dir_all(proc_root.join("self")).unwrap();

        let mut tree = descendants(&proc_root, 100);
        tree.sort();
        assert_eq!(
            tree,
            [(101, "bwrap".to_string()), (102, GSR_COMM.to_string())]
        );
        assert!(descendants(&proc_root, 102).is_empty());
        let _ = fs::remove_dir_all(&proc_root);
    }

    /// A stand-in for gsr's IPC server: answers each request with `reply(name)`.
    fn serve(socket: &Path, reply: fn(&str) -> String) {
        let listener = UnixListener::bind(socket).unwrap();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                let name = request["name"].as_str().unwrap_or_default().to_owned();
                let _ = (&stream).write_all(reply(&name).as_bytes());
            }
        });
    }

    #[test]
    fn requests_and_replies_follow_the_protocol() {
        assert_eq!(
            request_line(3, "save-replay", None),
            "{\"id\":3,\"name\":\"save-replay\"}\n"
        );
        assert_eq!(
            parse_reply("{\"id\":1,\"result\":\"ok\",\"data\":\"/v/Replay.mkv\"}\n").unwrap(),
            Some(Value::String("/v/Replay.mkv".into()))
        );
        assert_eq!(parse_reply("{\"id\":1,\"result\":\"ok\"}").unwrap(), None);
        let error = parse_reply("{\"id\":1,\"result\":\"error\",\"data\":\"no recording\"}");
        assert_eq!(error.unwrap_err().to_string(), "no recording");
        assert!(parse_reply("not json").is_err());

        let dir = temp_dir("ipc");
        let socket = dir.join("gsr.sock");
        serve(&socket, |name| match name {
            "save-replay" => "{\"id\":1,\"result\":\"ok\",\"data\":\"/v/Replay_1.mkv\"}\n".into(),
            "toggle-replay-recording" => "{\"id\":1,\"result\":\"ok\"}\n".into(),
            _ => "{\"id\":1,\"result\":\"error\",\"data\":\"unknown request name\"}\n".into(),
        });
        assert_eq!(
            save_replay(&socket, IPC_TIMEOUT).unwrap(),
            PathBuf::from("/v/Replay_1.mkv")
        );
        // The 6.1.6 name (`ipc_handlers.toggle_replay_recording` in src/cli/main.c, listed in the
        // man page's IPC section); the reply carries no data.
        assert_eq!(
            ipc_request(&socket, "toggle-replay-recording", None, IPC_TIMEOUT).unwrap(),
            None
        );
        let refused = ipc_request(&socket, "toggle-pause", None, IPC_TIMEOUT);
        assert_eq!(refused.unwrap_err().to_string(), "unknown request name");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ready_probe_waits_for_the_first_reply() {
        let dir = temp_dir("probe");
        let socket = dir.join("gsr.sock");
        let mut probe = ReadyProbe::new(&socket);
        // No socket yet: gsr has not started.
        assert!(!probe.poll());

        // Bound but not serving: the connection waits in the backlog, as with gsr before capture.
        let listener = UnixListener::bind(&socket).unwrap();
        assert!(!probe.poll());
        assert!(!probe.poll());

        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        assert!(line.contains(READY_PROBE));
        (&stream)
            .write_all(b"{\"id\":1,\"result\":\"error\",\"data\":\"unknown request name\"}\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !probe.poll() {
            assert!(Instant::now() < deadline, "the reply was never seen");
            thread::sleep(WAIT_STEP);
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
