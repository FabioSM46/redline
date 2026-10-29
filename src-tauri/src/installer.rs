//! The guided install of a missing tool (D4): `install_dependency(name)`.
//!
//! Redline never runs as root. It runs `pkexec` on one fixed program, the helper the `.deb`
//! installs at [`HELPER`], under its own polkit action (`com.fabiosm46.redline.install`, which
//! names that path as `org.freedesktop.policykit.exec.path`). The only argument is the
//! component's fixed name, from [`DependencyId`]: never a URL, a path or a command. `pkexec` is
//! used for nothing else.
//!
//! The helper writes `progress <sentence>` and `error <sentence>` lines on stdout (the package
//! managers' output goes to stderr). Each `progress` line becomes a `dependency-install` event
//! with `stage: "progress"`; the end is one event with `stage` `succeeded`, `failed` (with the
//! helper's own `error` sentence, otherwise the last line it wrote on stderr) or `cancelled`
//! (the password prompt was dismissed: pkexec's exit code 126, not an error). The command
//! resolves with that same final event once the helper has exited. One install at a time.

use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::dependencies::DependencyId;
use crate::recorder::Recorder;
use redline_core::thumbnail::find_program;

/// Where the `.deb` installs the helper (`bundle.linux.deb.files` in `tauri.conf.json`).
pub const HELPER: &str = "/usr/share/redline/install-dependency";
pub const INSTALL_EVENT: &str = "dependency-install";
const PKEXEC: &str = "pkexec";
/// pkexec's exit code when the user dismissed the authentication dialog.
const PKEXEC_DISMISSED: i32 = 126;
/// pkexec's exit code when authorization could not be obtained (or pkexec itself failed).
const PKEXEC_NOT_AUTHORIZED: i32 = 127;
/// The helper's exit code for "not supported on this system".
const HELPER_UNSUPPORTED: i32 = 3;
/// How many stderr lines are kept for a failure without an `error` line.
const STDERR_TAIL: usize = 20;

pub const NO_HELPER: &str = "The guided install comes with the Redline package. From a source \
    checkout, run `sudo bash scripts/install-deps.sh` instead.";
pub const NO_PKEXEC: &str = "The guided install needs pkexec (polkit), which is not installed. \
    Install polkit with your package manager, or install the tool by hand.";
pub const BUSY: &str = "Another install is running: wait for it to finish.";
const NOT_AUTHORIZED: &str = "Authentication failed, so nothing was installed.";

impl DependencyId {
    /// The component name the helper accepts.
    pub fn helper_arg(self) -> &'static str {
        match self {
            DependencyId::Lact => "lact",
            DependencyId::GpuScreenRecorder => "gpu-screen-recorder",
            DependencyId::Ffmpeg => "ffmpeg",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStage {
    Progress,
    Succeeded,
    Failed,
    Cancelled,
}

/// The payload of `dependency-install`, and the final result of `install_dependency`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallEvent {
    pub id: DependencyId,
    pub stage: InstallStage,
    /// `progress`: the step; `failed`: why; `succeeded` and `cancelled`: `null`.
    pub message: Option<String>,
}

/// Where the helper and pkexec are; tests pass fakes.
pub struct Installer {
    helper: PathBuf,
    search_path: std::ffi::OsString,
    busy: Arc<AtomicBool>,
}

impl Installer {
    pub fn new(helper: impl Into<PathBuf>, search_path: std::ffi::OsString) -> Self {
        Installer {
            helper: helper.into(),
            search_path,
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The packaged helper and the process PATH.
    pub fn system() -> Self {
        Installer::new(HELPER, std::env::var_os("PATH").unwrap_or_default())
    }

    /// `pkexec` and the helper are both there: the guided install can run here.
    pub fn available(&self) -> bool {
        self.ready().is_ok()
    }

    fn ready(&self) -> Result<PathBuf, &'static str> {
        if !self.helper.is_file() {
            return Err(NO_HELPER);
        }
        find_program(PKEXEC, &self.search_path).ok_or(NO_PKEXEC)
    }

    /// Takes the single install slot, or fails before anything runs.
    fn begin(&self) -> Result<(Job, BusyGuard), String> {
        let pkexec = self.ready().map_err(str::to_owned)?;
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(BUSY.to_owned());
        }
        let job = Job {
            pkexec,
            helper: self.helper.clone(),
        };
        Ok((job, BusyGuard(self.busy.clone())))
    }
}

/// Frees the install slot when the job ends, however it ends.
struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// One run of the helper through pkexec.
struct Job {
    pkexec: PathBuf,
    helper: PathBuf,
}

impl Job {
    /// Runs the install and reports each step to `emit`; returns the final event, which has
    /// also been emitted. Blocking: as long as the install takes.
    fn run(&self, id: DependencyId, emit: &dyn Fn(&InstallEvent)) -> InstallEvent {
        let event = |stage, message| InstallEvent { id, stage, message };
        let child = Command::new(&self.pkexec)
            .arg(&self.helper)
            .arg(id.helper_arg())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                let failed = event(
                    InstallStage::Failed,
                    Some(format!("Could not start pkexec ({error}).")),
                );
                emit(&failed);
                return failed;
            }
        };

        // stderr is read on its own thread, so a chatty package manager never fills the pipe.
        let stderr = child.stderr.take().map(|stderr| {
            thread::spawn(move || {
                let mut tail = std::collections::VecDeque::with_capacity(STDERR_TAIL);
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let line = line.trim().to_owned();
                    if line.is_empty() {
                        continue;
                    }
                    if tail.len() == STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
                tail
            })
        });
        let mut error_line = None;
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match parse_line(&line) {
                    Some(HelperLine::Progress(step)) => {
                        emit(&event(InstallStage::Progress, Some(step.to_owned())))
                    }
                    Some(HelperLine::Error(message)) => error_line = Some(message.to_owned()),
                    None => {}
                }
            }
        }
        let status = child.wait();
        let stderr_tail = stderr
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();

        let code = status.as_ref().ok().and_then(|status| status.code());
        let last = match code {
            Some(0) => event(InstallStage::Succeeded, None),
            Some(PKEXEC_DISMISSED) => event(InstallStage::Cancelled, None),
            Some(PKEXEC_NOT_AUTHORIZED) => {
                event(InstallStage::Failed, Some(NOT_AUTHORIZED.to_owned()))
            }
            _ => {
                let message = error_line
                    .or_else(|| stderr_tail.back().cloned())
                    .unwrap_or_else(|| match code {
                        Some(HELPER_UNSUPPORTED) => {
                            "The guided install does not support this system.".to_owned()
                        }
                        Some(code) => format!("The install failed (exit code {code})."),
                        None => "The install was interrupted.".to_owned(),
                    });
                event(InstallStage::Failed, Some(message))
            }
        };
        emit(&last);
        last
    }
}

#[derive(Debug, PartialEq, Eq)]
enum HelperLine<'a> {
    Progress(&'a str),
    Error(&'a str),
}

/// One stdout line of the helper; anything else is ignored.
fn parse_line(line: &str) -> Option<HelperLine<'_>> {
    let (kind, text) = line.trim_end().split_once(' ')?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match kind {
        "progress" => Some(HelperLine::Progress(text)),
        "error" => Some(HelperLine::Error(text)),
        _ => None,
    }
}

/// Registers the installer. Nothing runs until `install_dependency`.
pub fn init(app: &mut tauri::App) {
    app.manage(Installer::system());
}

/// Installs the tool `name` with the packaged helper through pkexec (one password prompt).
/// Fails before anything runs when the helper or pkexec is missing or another install is
/// running; after that, every outcome is a `dependency-install` event, and the command resolves
/// with the final one. A dismissed password prompt is `cancelled`, not an error.
#[tauri::command]
pub async fn install_dependency(
    app: AppHandle,
    installer: State<'_, Installer>,
    name: DependencyId,
) -> Result<InstallEvent, String> {
    let (job, guard) = installer.begin()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let emit = |event: &InstallEvent| {
            if let Err(error) = app.emit(INSTALL_EVENT, event) {
                eprintln!("redline: could not send {INSTALL_EVENT} ({error})");
            }
        };
        let result = job.run(name, &emit);
        // A recorder installed while Redline runs is picked up at once: replay becomes
        // available without a restart. LACT and ffmpeg are looked up on every request.
        if name == DependencyId::GpuScreenRecorder
            && result.stage == InstallStage::Succeeded
            && let Some(recorder) = app.try_state::<Recorder>()
        {
            recorder.detect();
        }
        result
    })
    .await
    .map_err(|error| format!("The install did not answer: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    struct Setup {
        dir: TempDir,
    }

    impl Setup {
        /// A fake pkexec that runs its arguments (or exits with `pkexec_code` without running
        /// them, like a dismissed prompt), and a fake helper with `helper_body`.
        fn new(name: &str, pkexec_code: Option<i32>, helper_body: &str) -> Self {
            let dir = TempDir::new("installer", name);
            fs::create_dir_all(dir.join("bin")).unwrap();
            let pkexec = match pkexec_code {
                Some(code) => format!("exit {code}"),
                None => "exec \"$@\"".to_owned(),
            };
            script(&dir.join("bin").join(PKEXEC), &pkexec);
            script(&dir.join("helper"), helper_body);
            Setup { dir }
        }

        fn installer(&self) -> Installer {
            Installer::new(
                self.dir.join("helper"),
                self.dir.join("bin").into_os_string(),
            )
        }
    }

    fn script(path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        // A test thread that forks while the file is still open for writing makes exec fail
        // with ETXTBSY until that child execs: wait until it can be run.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match Command::new(path)
                .arg("--probe")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                    assert!(Instant::now() < deadline, "{} stays busy", path.display());
                    thread::sleep(Duration::from_millis(10));
                }
                _ => break,
            }
        }
    }

    /// Runs one install and returns every emitted event and the result.
    fn install(installer: &Installer, id: DependencyId) -> (Vec<InstallEvent>, InstallEvent) {
        let (job, _guard) = installer.begin().unwrap();
        let events = Mutex::new(Vec::new());
        let result = job.run(id, &|event| events.lock().unwrap().push(event.clone()));
        (events.into_inner().unwrap(), result)
    }

    fn progress(id: DependencyId, step: &str) -> InstallEvent {
        InstallEvent {
            id,
            stage: InstallStage::Progress,
            message: Some(step.to_owned()),
        }
    }

    #[test]
    fn a_successful_install_reports_each_step_then_succeeded() {
        let setup = Setup::new(
            "success",
            None,
            "[ \"$1\" = --probe ] && exit 0\n\
             [ \"$1\" = ffmpeg ] || { echo \"error wrong component $1\"; exit 1; }\n\
             echo 'progress Installing ffmpeg'\n\
             echo 'Setting up ffmpeg ...' >&2\n\
             echo 'progress Done'",
        );
        let id = DependencyId::Ffmpeg;
        let (events, result) = install(&setup.installer(), id);
        let succeeded = InstallEvent {
            id,
            stage: InstallStage::Succeeded,
            message: None,
        };
        assert_eq!(
            events,
            vec![
                progress(id, "Installing ffmpeg"),
                progress(id, "Done"),
                succeeded.clone()
            ]
        );
        assert_eq!(result, succeeded);
    }

    #[test]
    fn each_component_reaches_the_helper_by_its_fixed_name() {
        let setup = Setup::new(
            "names",
            None,
            "[ $# -eq 1 ] || exit 1\necho \"progress got $1\"",
        );
        for (id, arg) in [
            (DependencyId::Lact, "lact"),
            (DependencyId::GpuScreenRecorder, "gpu-screen-recorder"),
            (DependencyId::Ffmpeg, "ffmpeg"),
        ] {
            let (events, result) = install(&setup.installer(), id);
            assert_eq!(events[0], progress(id, &format!("got {arg}")));
            assert_eq!(result.stage, InstallStage::Succeeded);
        }
    }

    #[test]
    fn a_failure_keeps_the_helper_message() {
        let setup = Setup::new(
            "failure",
            None,
            "echo 'progress Checking the LACT package'\n\
             echo 'some apt noise' >&2\n\
             echo 'error The downloaded LACT package does not match its pinned checksum: nothing was installed.'\n\
             exit 1",
        );
        let (events, result) = install(&setup.installer(), DependencyId::Lact);
        assert_eq!(events.len(), 2);
        assert_eq!(result.stage, InstallStage::Failed);
        assert_eq!(
            result.message.as_deref(),
            Some(
                "The downloaded LACT package does not match its pinned checksum: nothing was installed."
            )
        );
        assert_eq!(events.last(), Some(&result));
    }

    #[test]
    fn without_an_error_line_the_last_stderr_line_explains() {
        let setup = Setup::new(
            "stderr",
            None,
            "echo 'E: Unable to locate package ffmpeg' >&2\necho >&2\nexit 100",
        );
        let (_, result) = install(&setup.installer(), DependencyId::Ffmpeg);
        assert_eq!(result.stage, InstallStage::Failed);
        assert_eq!(
            result.message.as_deref(),
            Some("E: Unable to locate package ffmpeg")
        );
    }

    #[test]
    fn a_dismissed_password_prompt_is_cancelled_not_an_error() {
        let setup = Setup::new("cancelled", Some(PKEXEC_DISMISSED), "exit 0");
        let (events, result) = install(&setup.installer(), DependencyId::Lact);
        assert_eq!(
            result,
            InstallEvent {
                id: DependencyId::Lact,
                stage: InstallStage::Cancelled,
                message: None
            }
        );
        assert_eq!(events, vec![result]);
    }

    #[test]
    fn a_failed_authentication_says_nothing_was_installed() {
        let setup = Setup::new("not-authorized", Some(PKEXEC_NOT_AUTHORIZED), "exit 0");
        let (_, result) = install(&setup.installer(), DependencyId::Lact);
        assert_eq!(result.stage, InstallStage::Failed);
        assert_eq!(result.message.as_deref(), Some(NOT_AUTHORIZED));
    }

    #[test]
    fn one_install_at_a_time() {
        let setup = Setup::new("busy", None, "exit 0");
        let installer = setup.installer();
        let first = installer.begin().unwrap();
        assert_eq!(installer.begin().err().as_deref(), Some(BUSY));
        drop(first);
        assert!(installer.begin().is_ok());
    }

    #[test]
    fn without_the_helper_or_pkexec_nothing_runs() {
        let setup = Setup::new("missing", None, "exit 0");
        let no_helper = Installer::new(
            setup.dir.join("absent"),
            setup.dir.join("bin").into_os_string(),
        );
        assert_eq!(no_helper.begin().err().as_deref(), Some(NO_HELPER));
        assert!(!no_helper.available());
        let no_pkexec = Installer::new(setup.dir.join("helper"), "/nonexistent".into());
        assert_eq!(no_pkexec.begin().err().as_deref(), Some(NO_PKEXEC));
        assert!(setup.installer().available());
    }

    #[test]
    fn parses_only_the_two_kinds_of_lines() {
        assert_eq!(
            parse_line("progress Installing ffmpeg\n"),
            Some(HelperLine::Progress("Installing ffmpeg"))
        );
        assert_eq!(parse_line("error Boom"), Some(HelperLine::Error("Boom")));
        assert_eq!(parse_line("run apt-get update"), None);
        assert_eq!(parse_line("progress "), None);
        assert_eq!(parse_line("progress"), None);
    }

    #[test]
    fn serializes_the_event() {
        let json =
            serde_json::to_value(progress(DependencyId::GpuScreenRecorder, "Adding Flathub"))
                .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": "gpu_screen_recorder", "stage": "progress", "message": "Adding Flathub"
            })
        );
    }
}
