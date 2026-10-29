//! The external tools Redline uses, and whether each one is there: `dependency_status()`.
//!
//! - **LACT** (tuning): the `lact` binary on PATH, and whether `lactd` runs and its socket can be
//!   opened, through the tuning service's [`Lact`] (the same detection and sentences as
//!   `tuning_status`). The version is the one the daemon reports, so it is known only while
//!   `lactd` answers.
//! - **GPU Screen Recorder** (replay and recording): the native binary on PATH, otherwise the
//!   Flatpak, in the order the recorder uses them. The version is `--version` of the native binary
//!   or the `Version:` line of `flatpak info`.
//! - **ffmpeg** (previews of clips): on PATH, with the version from `ffmpeg -version`. Redline
//!   verifies no ffmpeg version, so `verified_version` is `null`.
//!
//! A missing tool is not an error: `installed: false` with a `hint` that says what to do. Every
//! tool is run with a timeout, from a blocking thread, never from the main thread.

use redline_core::lact::LactError;
use redline_core::recorder::{self as gsr, FLATPAK_ID};
use redline_core::thumbnail::{self, find_program};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::State;

use crate::installer::Installer;
use crate::recorder::MISSING as GSR_MISSING;
use crate::tuning::{self, Lact, Tuning};

/// How long a tool may take to print its version.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(5);
const FLATPAK: &str = "flatpak";
const FFMPEG: &str = "ffmpeg";
/// `flatpak info` did not answer in time (or could not run), so whether the recorder is
/// installed as a Flatpak is unknown.
pub const FLATPAK_UNANSWERED: &str = "Could not check whether GPU Screen Recorder is installed: \
    Flatpak did not answer in time. Check again before installing it.";
/// ffmpeg is on PATH but `ffmpeg -version` failed, hung or printed something unexpected.
pub const FFMPEG_UNREADABLE: &str =
    "Could not read the ffmpeg version: previews of clips may not work.";

/// The tools Redline depends on, in the order `dependency_status` lists them. The same ids name
/// them for the guided install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyId {
    Lact,
    GpuScreenRecorder,
    Ffmpeg,
}

impl DependencyId {
    /// The name shown to the user.
    pub fn name(self) -> &'static str {
        match self {
            DependencyId::Lact => "LACT",
            DependencyId::GpuScreenRecorder => "GPU Screen Recorder",
            DependencyId::Ffmpeg => "ffmpeg",
        }
    }
}

/// One tool, as `dependency_status` returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dependency {
    pub id: DependencyId,
    pub name: &'static str,
    pub installed: bool,
    /// The installed version; `null` when unknown (LACT: while `lactd` does not answer).
    pub version: Option<String>,
    /// The version Redline is verified with; `null` when Redline verifies none (ffmpeg).
    pub verified_version: Option<&'static str>,
    /// A notice that blocks nothing, such as an unverified version.
    pub warning: Option<String>,
    /// What to do: install the tool, or (LACT) start its daemon or join its admin group. `null`
    /// when nothing is missing.
    pub hint: Option<String>,
    /// LACT only: its daemon. `null` for the other tools.
    pub daemon: Option<DaemonStatus>,
    /// `install_dependency` can run here: the packaged helper and pkexec are both present. The
    /// same for every tool; the helper itself says when a tool is not supported on the system.
    pub can_install: bool,
}

/// `lactd`, as seen from Redline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DaemonStatus {
    /// Something listens on the socket (it may still refuse this user).
    pub running: bool,
    /// Redline can open the socket and `lactd` answered. False with `running` true: the user is
    /// not in lactd's admin group, or the daemon did not answer in time; `hint` says which.
    pub socket_accessible: bool,
}

/// Every dependency: LACT, GPU Screen Recorder, ffmpeg. Blocking: the three checks run side by
/// side (one request to `lactd`, bounded by the LACT client's own timeout, and up to three tool
/// runs, each bounded by `timeout`), so the worst case is about one timeout, not their sum.
pub fn status(
    lact: &Lact,
    search_path: &OsStr,
    timeout: Duration,
    can_install: bool,
) -> Vec<Dependency> {
    let mut deps = thread::scope(|scope| {
        let recorder = scope.spawn(|| gpu_screen_recorder_status(search_path, timeout));
        let ffmpeg = scope.spawn(|| ffmpeg_status(search_path, timeout));
        let lact = lact_status(lact);
        vec![
            lact,
            recorder.join().expect("the recorder check panicked"),
            ffmpeg.join().expect("the ffmpeg check panicked"),
        ]
    });
    for dep in &mut deps {
        dep.can_install = can_install;
    }
    deps
}

fn missing(id: DependencyId, verified_version: Option<&'static str>, hint: &str) -> Dependency {
    Dependency {
        id,
        name: id.name(),
        installed: false,
        version: None,
        verified_version,
        warning: None,
        hint: Some(hint.to_owned()),
        daemon: None,
        can_install: false,
    }
}

fn found(
    id: DependencyId,
    version: Option<String>,
    verified_version: Option<&'static str>,
) -> Dependency {
    Dependency {
        id,
        name: id.name(),
        installed: true,
        version,
        verified_version,
        warning: None,
        hint: None,
        daemon: None,
        can_install: false,
    }
}

fn lact_status(lact: &Lact) -> Dependency {
    let id = DependencyId::Lact;
    let verified = Some(redline_core::lact::VERIFIED_VERSION);
    match lact.system_info() {
        // A daemon that answers is an installed LACT, wherever its binary is.
        Ok(system) => Dependency {
            warning: tuning::version_notice(&system.version),
            daemon: Some(DaemonStatus {
                running: true,
                socket_accessible: true,
            }),
            ..found(id, Some(system.version), verified)
        },
        Err(error) => {
            // Every variant named, so a new one has to be placed here on purpose. `hint` always
            // says which case it is: `running` with no access is either a denied socket or a
            // daemon that did not answer in time (`Io`).
            let (running, socket_accessible) = match &error {
                LactError::SocketMissing | LactError::NotRunning => (false, false),
                LactError::PermissionDenied | LactError::Io(_) => (true, false),
                LactError::Protocol(_) | LactError::Daemon(_) => (true, true),
            };
            Dependency {
                installed: lact.installed(),
                hint: Some(lact.reason(&error)),
                daemon: Some(DaemonStatus {
                    running,
                    socket_accessible,
                }),
                ..missing(id, verified, "")
            }
        }
    }
}

fn gpu_screen_recorder_status(search_path: &OsStr, timeout: Duration) -> Dependency {
    let id = DependencyId::GpuScreenRecorder;
    let verified = Some(gsr::VERIFIED_VERSION);
    let version = if let Some(native) = find_program(gsr::NATIVE_BINARY, search_path) {
        run(&native, &["--version"], timeout).map(|out| first_line(&out))
    } else if let Some(flatpak) = find_program(FLATPAK, search_path) {
        match run_outcome(&flatpak, &["info", FLATPAK_ID], timeout) {
            Run::Output(out) => Some(flatpak_version(&out)),
            // flatpak's own answer: the app is not installed.
            Run::Exited => return missing(id, verified, GSR_MISSING),
            // No answer is not "missing": say so, rather than send the user to install it again.
            Run::Unanswered => return missing(id, verified, FLATPAK_UNANSWERED),
        }
    } else {
        return missing(id, verified, GSR_MISSING);
    };
    // Found, but `--version` failed: installed with an unknown version, as the recorder says.
    let version = version.flatten();
    Dependency {
        warning: gsr::version_notice(version.as_deref()),
        ..found(id, version, verified)
    }
}

fn ffmpeg_status(search_path: &OsStr, timeout: Duration) -> Dependency {
    let id = DependencyId::Ffmpeg;
    match find_program(FFMPEG, search_path) {
        Some(ffmpeg) => {
            let version = run(&ffmpeg, &["-version"], timeout).and_then(|out| ffmpeg_version(&out));
            Dependency {
                warning: version.is_none().then(|| FFMPEG_UNREADABLE.to_owned()),
                ..found(id, version, None)
            }
        }
        None => missing(id, None, thumbnail::INSTALL_HINT),
    }
}

/// The first non-empty line, trimmed, as gsr prints its version.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// The value of the `Version:` line of `flatpak info` (run with `LC_ALL=C`, so not translated).
fn flatpak_version(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("Version:"))
        .map(str::trim)
        .find(|version| !version.is_empty())
        .map(str::to_owned)
}

/// `ffmpeg version 6.1.1-3ubuntu5 Copyright …` → `6.1.1-3ubuntu5`.
fn ffmpeg_version(text: &str) -> Option<String> {
    let line = first_line(text)?;
    let mut words = line.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("ffmpeg"), Some("version"), Some(version)) => Some(version.to_owned()),
        _ => None,
    }
}

/// How a tool run ended.
#[derive(Debug, PartialEq, Eq)]
enum Run {
    /// Exited with success; its stdout.
    Output(String),
    /// Exited with a failure status: the tool's own answer.
    Exited,
    /// Could not be started, was killed after the timeout, or its output could not be read.
    Unanswered,
}

/// Runs `program` with `args` in the C locale and returns its stdout when it exits with
/// success within `timeout`; after that it is killed and reaped. `None` for a failure.
fn run(program: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    match run_outcome(program, args, timeout) {
        Run::Output(text) => Some(text),
        Run::Exited | Run::Unanswered => None,
    }
}

/// [`run`], telling a tool that answered with a failure from one that did not answer.
fn run_outcome(program: &Path, args: &[&str], timeout: Duration) -> Run {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Run::Unanswered;
    };
    // Read while waiting, so a long output cannot fill the pipe and stall the tool. The text
    // comes back over a channel: a process the tool leaves behind may keep the pipe open, and
    // the reader is then never joined, only abandoned.
    let Some(mut stdout) = child.stdout.take() else {
        return Run::Unanswered;
    };
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut text = String::new();
        let _ = sender.send(stdout.read_to_string(&mut text).ok().map(|_| text));
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return Run::Exited,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Run::Unanswered;
            }
        }
    }
    // The tool has exited: its output is complete unless something it started still holds the
    // pipe, and that is waited for only until the deadline (plus a moment for the last read).
    let left = deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(100);
    match receiver.recv_timeout(left).ok().flatten() {
        Some(text) => Run::Output(text),
        None => Run::Unanswered,
    }
}

/// The search path of this process.
fn process_path() -> OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

/// LACT, GPU Screen Recorder and ffmpeg, in that order. Never an error for a missing tool:
/// that is `installed: false` with a hint.
#[tauri::command]
pub async fn dependency_status(
    tuning: State<'_, Tuning>,
    installer: State<'_, Installer>,
) -> Result<Vec<Dependency>, String> {
    let tuning = tuning.inner().clone();
    let can_install = installer.available();
    tauri::async_runtime::spawn_blocking(move || {
        status(tuning.lact(), &process_path(), TOOL_TIMEOUT, can_install)
    })
    .await
    .map_err(|error| format!("The dependency check did not answer: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use crate::tuning::{NO_PERMISSION, NOT_INSTALLED, NOT_RUNNING};
    use redline_core::lact::{LactClient, VERIFIED_VERSION as LACT_VERIFIED};
    use serde_json::{Value, json};
    use std::fs;
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    const TIMEOUT: Duration = Duration::from_secs(2);

    struct Setup {
        dir: TempDir,
    }

    impl Setup {
        fn new(name: &str) -> Self {
            let dir = TempDir::new("dependencies", name);
            fs::create_dir_all(dir.join("bin")).unwrap();
            Setup { dir }
        }

        fn bin(&self) -> PathBuf {
            self.dir.join("bin")
        }

        fn socket(&self) -> PathBuf {
            self.dir.join("lactd.sock")
        }

        /// A shell script called `name` on the fake PATH.
        fn tool(&self, name: &str, body: &str) {
            let path = self.bin().join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            // A test thread that forks while the file is still open for writing makes exec fail
            // with ETXTBSY until that child execs: wait for the script to run once.
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match Command::new(&path)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                {
                    Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                        assert!(Instant::now() < deadline, "{name} stays busy");
                        thread::sleep(Duration::from_millis(10));
                    }
                    _ => break,
                }
            }
        }

        fn lact(&self) -> Lact {
            Lact::new(
                LactClient::new(self.socket()).with_timeout(TIMEOUT),
                self.bin().into_os_string(),
            )
        }

        fn status(&self) -> Vec<Dependency> {
            status(&self.lact(), self.bin().as_os_str(), TIMEOUT, true)
        }

        fn get(&self, id: DependencyId) -> Dependency {
            self.status().into_iter().find(|dep| dep.id == id).unwrap()
        }
    }

    /// A `lactd` that answers one request with `system_info` of `version`.
    fn fake_lactd(socket: &Path, version: &'static str) -> thread::JoinHandle<()> {
        let listener = UnixListener::bind(socket).unwrap();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["command"], "system_info");
            let answer = json!({ "status": "ok", "data": { "version": version } });
            writeln!(&stream, "{answer}").unwrap();
        })
    }

    #[test]
    fn with_nothing_installed_every_tool_says_what_to_install() {
        let setup = Setup::new("nothing");
        let deps = setup.status();
        let ids: Vec<_> = deps.iter().map(|dep| dep.id).collect();
        assert_eq!(
            ids,
            [
                DependencyId::Lact,
                DependencyId::GpuScreenRecorder,
                DependencyId::Ffmpeg
            ]
        );
        assert!(
            deps.iter()
                .all(|dep| !dep.installed && dep.version.is_none() && dep.can_install)
        );

        assert_eq!(deps[0].hint.as_deref(), Some(NOT_INSTALLED));
        assert_eq!(deps[0].verified_version, Some(LACT_VERIFIED));
        assert_eq!(
            deps[0].daemon,
            Some(DaemonStatus {
                running: false,
                socket_accessible: false
            })
        );
        assert_eq!(deps[1].hint.as_deref(), Some(GSR_MISSING));
        assert_eq!(deps[1].verified_version, Some(gsr::VERIFIED_VERSION));
        assert_eq!(deps[1].daemon, None);
        assert_eq!(deps[2].hint.as_deref(), Some(thumbnail::INSTALL_HINT));
        assert_eq!(deps[2].verified_version, None);
    }

    #[test]
    fn lact_installed_without_its_daemon_says_to_start_it() {
        let setup = Setup::new("lact-stopped");
        setup.tool("lact", "exit 0");
        let lact = setup.get(DependencyId::Lact);
        assert!(lact.installed);
        assert_eq!(lact.version, None);
        assert_eq!(lact.hint.as_deref(), Some(NOT_RUNNING));
        assert_eq!(lact.daemon.map(|d| d.running), Some(false));

        // A socket left behind by a stopped daemon reads the same. Another test thread that
        // forks a fake tool holds the listener until its exec (and a connection to it is then
        // reset): wait for that copy to go.
        drop(UnixListener::bind(setup.socket()).unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        let lact = loop {
            let lact = setup.get(DependencyId::Lact);
            if lact.hint.as_deref() == Some(NOT_RUNNING) || Instant::now() > deadline {
                break lact;
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(lact.hint.as_deref(), Some(NOT_RUNNING));
        assert_eq!(lact.daemon.map(|d| d.running), Some(false));
    }

    #[test]
    fn a_running_lactd_gives_the_version() {
        let setup = Setup::new("lact-running");
        setup.tool("lact", "exit 0");
        let lactd = fake_lactd(&setup.socket(), LACT_VERIFIED);
        let lact = setup.get(DependencyId::Lact);
        lactd.join().unwrap();
        assert!(lact.installed);
        assert_eq!(lact.version.as_deref(), Some(LACT_VERIFIED));
        assert_eq!((lact.hint, lact.warning), (None, None));
        assert_eq!(
            lact.daemon,
            Some(DaemonStatus {
                running: true,
                socket_accessible: true
            })
        );
    }

    #[test]
    fn another_lact_version_warns() {
        let setup = Setup::new("lact-version");
        let lactd = fake_lactd(&setup.socket(), "0.11.0");
        let lact = setup.get(DependencyId::Lact);
        lactd.join().unwrap();
        // The daemon answers: installed even without `lact` on this PATH.
        assert!(lact.installed);
        assert_eq!(lact.version.as_deref(), Some("0.11.0"));
        let warning = lact.warning.unwrap();
        assert!(warning.contains("0.11.0") && warning.contains(LACT_VERIFIED));
    }

    #[test]
    fn a_socket_the_user_may_not_open_is_running_but_not_accessible() {
        // root opens any socket: the check means nothing there.
        if fs::metadata("/proc/self").is_ok_and(|meta| meta.uid() == 0) {
            return;
        }
        let setup = Setup::new("lact-permission");
        setup.tool("lact", "exit 0");
        let _listener = UnixListener::bind(setup.socket()).unwrap();
        fs::set_permissions(setup.socket(), fs::Permissions::from_mode(0o000)).unwrap();
        let lact = setup.get(DependencyId::Lact);
        assert!(lact.installed);
        assert_eq!(lact.hint.as_deref(), Some(NO_PERMISSION));
        assert_eq!(
            lact.daemon,
            Some(DaemonStatus {
                running: true,
                socket_accessible: false
            })
        );
    }

    #[test]
    fn the_native_recorder_comes_first_with_its_version() {
        let setup = Setup::new("gsr-native");
        setup.tool(
            gsr::NATIVE_BINARY,
            &format!("[ \"$1\" = --version ] && echo {}", gsr::VERIFIED_VERSION),
        );
        // Never asked: the native binary wins.
        setup.tool(FLATPAK, "exit 1");
        let dep = setup.get(DependencyId::GpuScreenRecorder);
        assert!(dep.installed);
        assert_eq!(dep.version.as_deref(), Some(gsr::VERIFIED_VERSION));
        assert_eq!((dep.hint, dep.warning), (None, None));
    }

    #[test]
    fn the_flatpak_recorder_reads_its_version_from_flatpak_info() {
        let setup = Setup::new("gsr-flatpak");
        setup.tool(
            FLATPAK,
            &format!(
                "[ \"$1 $2\" = 'info {FLATPAK_ID}' ] || exit 1\n\
                 [ \"$LC_ALL\" = C ] || exit 1\n\
                 printf '\\nGPU Screen Recorder - Record your screen\\n\\n          ID: {FLATPAK_ID}\\n         Ref: app/{FLATPAK_ID}/x86_64/stable\\n     Version: 5.12.0\\n'"
            ),
        );
        let dep = setup.get(DependencyId::GpuScreenRecorder);
        assert!(dep.installed);
        assert_eq!(dep.version.as_deref(), Some("5.12.0"));
        assert!(dep.warning.unwrap().contains("5.12.0"));
    }

    #[test]
    fn flatpak_without_the_recorder_is_missing() {
        let setup = Setup::new("gsr-no-app");
        setup.tool(FLATPAK, "echo 'error: com.dec05eba.gpu_screen_recorder/*unspecified*/*unspecified* not installed' >&2; exit 1");
        let dep = setup.get(DependencyId::GpuScreenRecorder);
        assert!(!dep.installed);
        assert_eq!(dep.hint.as_deref(), Some(GSR_MISSING));
    }

    #[test]
    fn a_flatpak_that_does_not_answer_is_not_called_missing() {
        let setup = Setup::new("gsr-flatpak-hangs");
        setup.tool(FLATPAK, "[ \"$1\" = info ] && exec sleep 30");
        let dep = status(
            &setup.lact(),
            setup.bin().as_os_str(),
            Duration::from_millis(200),
            false,
        )
        .remove(1);
        assert!(!dep.installed);
        assert_eq!(dep.hint.as_deref(), Some(FLATPAK_UNANSWERED));
    }

    #[test]
    fn a_recorder_whose_version_fails_is_installed_with_a_warning() {
        let setup = Setup::new("gsr-broken");
        setup.tool(gsr::NATIVE_BINARY, "exit 1");
        let dep = setup.get(DependencyId::GpuScreenRecorder);
        assert!(dep.installed);
        assert_eq!(dep.version, None);
        assert!(dep.warning.unwrap().contains("Could not read"));
    }

    #[test]
    fn ffmpeg_reports_its_version() {
        let setup = Setup::new("ffmpeg");
        setup.tool(
            FFMPEG,
            "[ \"$1\" = -version ] && echo 'ffmpeg version 6.1.1-3ubuntu5 Copyright (c) 2000-2023 the FFmpeg developers'",
        );
        let dep = setup.get(DependencyId::Ffmpeg);
        assert!(dep.installed);
        assert_eq!(dep.version.as_deref(), Some("6.1.1-3ubuntu5"));
        assert_eq!(
            (dep.hint, dep.warning, dep.verified_version),
            (None, None, None)
        );
    }

    #[test]
    fn an_ffmpeg_whose_version_cannot_be_read_warns() {
        let setup = Setup::new("ffmpeg-broken");
        setup.tool(FFMPEG, "exit 1");
        let dep = setup.get(DependencyId::Ffmpeg);
        assert!(dep.installed);
        assert_eq!(dep.version, None);
        assert_eq!(dep.warning.as_deref(), Some(FFMPEG_UNREADABLE));
    }

    #[test]
    fn a_process_left_holding_the_output_does_not_stall_the_check() {
        let setup = Setup::new("ffmpeg-child");
        // Exits at once, but the background `sleep` keeps the inherited stdout open.
        setup.tool(
            FFMPEG,
            "[ \"$1\" = -version ] || exit 0\nsleep 3 &\necho 'ffmpeg version 7.1'",
        );
        let started = Instant::now();
        let dep = status(
            &setup.lact(),
            setup.bin().as_os_str(),
            Duration::from_millis(300),
            false,
        )
        .remove(2);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(dep.installed);
    }

    #[test]
    fn the_checks_run_side_by_side() {
        let setup = Setup::new("parallel");
        setup.tool(FFMPEG, "[ \"$1\" = -version ] && exec sleep 30");
        setup.tool(
            gsr::NATIVE_BINARY,
            "[ \"$1\" = --version ] && exec sleep 30",
        );
        let timeout = Duration::from_secs(1);
        let started = Instant::now();
        let deps = status(&setup.lact(), setup.bin().as_os_str(), timeout, false);
        // Two hanging tools one after the other would take two timeouts.
        assert!(started.elapsed() < timeout * 2, "{:?}", started.elapsed());
        assert!(deps[1].installed && deps[2].installed);
    }

    #[test]
    fn a_tool_that_hangs_is_killed_after_the_timeout() {
        let setup = Setup::new("ffmpeg-hangs");
        setup.tool(FFMPEG, "[ \"$1\" = -version ] && exec sleep 30");
        let started = Instant::now();
        let dep = status(
            &setup.lact(),
            setup.bin().as_os_str(),
            Duration::from_millis(200),
            false,
        )
        .remove(2);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(dep.installed);
        assert_eq!(dep.version, None);
    }

    #[test]
    fn parses_versions() {
        assert_eq!(
            ffmpeg_version("ffmpeg version n7.1 Copyright"),
            Some("n7.1".into())
        );
        assert_eq!(ffmpeg_version("something else"), None);
        assert_eq!(
            flatpak_version("  Version: 6.1.6\n  Branch: stable"),
            Some("6.1.6".into())
        );
        assert_eq!(flatpak_version("  Branch: stable"), None);
        assert_eq!(first_line("\n  6.1.6  \nmore"), Some("6.1.6".into()));
    }

    #[test]
    fn serializes_the_contract() {
        let json =
            serde_json::to_value(missing(DependencyId::GpuScreenRecorder, None, "x")).unwrap();
        assert_eq!(json["id"], "gpu_screen_recorder");
        assert_eq!(json["name"], "GPU Screen Recorder");
        assert_eq!(json["can_install"], false);
        for key in ["version", "verified_version", "warning", "daemon"] {
            assert_eq!(json[key], Value::Null, "{key}");
        }
        let id: DependencyId = serde_json::from_value(json!("ffmpeg")).unwrap();
        assert_eq!(id, DependencyId::Ffmpeg);
    }
}
