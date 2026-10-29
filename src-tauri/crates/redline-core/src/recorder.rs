//! Control of GPU Screen Recorder (gsr) as an external process.
//!
//! gsr is driven with signals and writes the path of every saved file to stdout:
//! - SIGUSR1   saves the replay
//! - SIGRTMIN  starts/stops a regular recording while the replay is running (`-ro`)
//! - SIGINT    stops gsr
//!
//! We use the native binary when it is on PATH, otherwise the Flatpak.

use anyhow::{Context, Result, bail};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

const FLATPAK_ID: &str = "com.dec05eba.gpu_screen_recorder";
/// The process `comm` in /proc, truncated by the kernel to 15 characters.
const GSR_COMM: &str = "gpu-screen-reco";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    Native,
    Flatpak,
}

impl Launcher {
    pub fn detect() -> Result<Launcher> {
        if Command::new("gpu-screen-recorder")
            .arg("--version")
            .output()
            .is_ok()
        {
            return Ok(Launcher::Native);
        }
        let flatpak = Command::new("flatpak").args(["info", FLATPAK_ID]).output();
        if matches!(flatpak, Ok(out) if out.status.success()) {
            return Ok(Launcher::Flatpak);
        }
        bail!("GPU Screen Recorder not found (neither on PATH nor as Flatpak {FLATPAK_ID})")
    }

    fn command(self) -> Command {
        match self {
            Launcher::Native => Command::new("gpu-screen-recorder"),
            Launcher::Flatpak => {
                let mut cmd = Command::new("flatpak");
                cmd.args(["run", "--command=gpu-screen-recorder", FLATPAK_ID]);
                cmd
            }
        }
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

/// gsr in replay mode, started and owned by us.
pub struct ReplaySession {
    child: Child,
    launcher: Launcher,
    /// Paths of the saved files, read from gsr's stdout
    pub saved: Receiver<PathBuf>,
}

impl ReplaySession {
    pub fn start(launcher: Launcher, config: &ReplayConfig) -> Result<ReplaySession> {
        fs::create_dir_all(&config.replay_dir)?;
        fs::create_dir_all(&config.recording_dir)?;

        let mut child = launcher
            .command()
            .args(config.args())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("starting gpu-screen-recorder failed")?;

        let stdout = child.stdout.take().expect("stdout in pipe");
        let (tx, saved) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let line = line.trim();
                if !line.is_empty() && tx.send(PathBuf::from(line)).is_err() {
                    break;
                }
            }
        });

        Ok(ReplaySession {
            child,
            launcher,
            saved,
        })
    }

    pub fn save_replay(&self) -> Result<()> {
        self.signal(libc::SIGUSR1)
    }

    pub fn toggle_recording(&self) -> Result<()> {
        self.signal(libc::SIGRTMIN())
    }

    /// After stopping, `saved` stays readable for the last paths gsr writes.
    pub fn stop(&mut self) -> Result<()> {
        self.signal(libc::SIGINT)?;
        self.child.wait()?;
        Ok(())
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn signal(&self, sig: libc::c_int) -> Result<()> {
        let pids = match self.launcher {
            Launcher::Native => vec![self.child.id() as i32],
            // `flatpak run` is not guaranteed to forward signals: find the real gsr process.
            // The prototype takes every gsr of the user; the app will use `-ipc`.
            Launcher::Flatpak => find_gsr_pids(),
        };
        if pids.is_empty() {
            bail!("gpu-screen-recorder process not found");
        }
        for pid in pids {
            // SAFETY: kill() touches no memory, at worst it fails with errno
            if unsafe { libc::kill(pid, sig) } != 0 {
                bail!("kill({pid}) failed: {}", std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

fn find_gsr_pids() -> Vec<i32> {
    let uid = unsafe { libc::getuid() };
    let Ok(entries) = fs::read_dir("/proc") else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
            let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
            let owner = status
                .lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|l| l.split_whitespace().next()?.parse::<u32>().ok());
            comm.trim() == GSR_COMM && owner == Some(uid)
        })
        .collect()
}

/// Audio source listed by gsr: (name to pass to `-a`, description).
pub fn list_audio_devices(launcher: Launcher) -> Result<Vec<(String, String)>> {
    let out = launcher.command().arg("--list-audio-devices").output()?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (name, desc) = l.split_once('|')?;
            Some((name.to_owned(), desc.to_owned()))
        })
        .collect())
}

/// Applications currently playing audio, usable as `app:<name>`.
pub fn list_application_audio(launcher: Launcher) -> Result<Vec<String>> {
    let out = launcher
        .command()
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
}
