//! CLI to try the pieces of redline-core by hand before building the UI on top.

use anyhow::Result;
use clap::{Parser, Subcommand};
use redline_core::lact::LactClient;
use redline_core::recorder::{self, AudioTrack, Launcher, ReplayConfig, ReplaySession};
use redline_core::sensors::{AmdGpu, GpuSnapshot};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

#[derive(Parser)]
#[command(about = "Redline integration probes")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// GPU sensors read from sysfs
    Sensors {
        /// Refresh every second
        #[arg(long)]
        watch: bool,
        #[arg(long)]
        json: bool,
    },
    /// Query the LACT daemon
    Lact {
        #[arg(value_parser = ["ping", "devices", "info", "stats"])]
        what: String,
    },
    /// Audio sources available to GPU Screen Recorder
    Audio,
    /// Interactive replay buffer: [s] save, [r] start/stop recording, [q] quit
    Replay {
        #[arg(long, default_value_t = 60)]
        seconds: u32,
        #[arg(long, default_value_t = 60)]
        fps: u32,
        #[arg(long, default_value = "portal")]
        capture: String,
        /// One track per value; use `|` to mix several sources into the same track
        #[arg(long = "track", default_values_t = ["default_output".to_string(), "default_input".to_string()])]
        tracks: Vec<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// List the (video/audio) streams of a file with ffprobe
    Tracks { file: PathBuf },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Sensors { watch, json } => sensors(watch, json),
        Cmd::Lact { what } => lact(&what),
        Cmd::Audio => audio(),
        Cmd::Replay {
            seconds,
            fps,
            capture,
            tracks,
            out,
        } => replay(seconds, fps, capture, tracks, out),
        Cmd::Tracks { file } => tracks(&file),
    }
}

fn sensors(watch: bool, json: bool) -> Result<()> {
    let gpu = AmdGpu::first()?;
    eprintln!("GPU {} ({})", gpu.pci_slot, gpu.device.display());
    loop {
        let s = gpu.snapshot();
        if json {
            println!("{}", serde_json::to_string(&s)?);
        } else {
            println!("{}", format_snapshot(&s));
        }
        if !watch {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn format_snapshot(s: &GpuSnapshot) -> String {
    fn o<T: std::fmt::Display>(v: Option<T>) -> String {
        v.map_or("-".into(), |v| v.to_string())
    }
    format!(
        "edge {}°C | hotspot {}°C | mem {}°C | {:.0}/{}W | fan {} rpm | core {} MHz | vram clk {} MHz | {} mV | GPU {}% | VRAM {}/{} MiB",
        o(s.temp_edge_c),
        o(s.temp_junction_c),
        o(s.temp_mem_c),
        s.power_w.unwrap_or(0.0),
        o(s.power_cap_w.map(|w| w.round())),
        o(s.fan_rpm),
        o(s.sclk_mhz),
        o(s.mclk_mhz),
        o(s.vddgfx_mv),
        o(s.gpu_busy_pct),
        o(s.vram_used_mib),
        o(s.vram_total_mib),
    )
}

fn lact(what: &str) -> Result<()> {
    let client = LactClient::default();
    let id = || -> Result<String> {
        // LACT identifies the GPU as "VENDOR:DEVICE-SUBVENDOR:SUBDEVICE-PCI_SLOT"
        let devices = client.list_devices()?;
        Ok(devices[0]["id"].as_str().unwrap_or_default().to_owned())
    };
    let out = match what {
        "ping" => {
            client.ping()?;
            serde_json::json!("pong")
        }
        "devices" => client.list_devices()?,
        "info" => client.device_info(&id()?)?,
        "stats" => client.device_stats(&id()?)?,
        _ => unreachable!(),
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn audio() -> Result<()> {
    let launcher = Launcher::detect()?;
    println!("Devices ({launcher:?}):");
    for (name, desc) in recorder::list_audio_devices(launcher)? {
        println!("  {name:<40} {desc}");
    }
    println!("Applications (use app:<name>):");
    for app in recorder::list_application_audio(launcher)? {
        println!("  {app}");
    }
    Ok(())
}

fn replay(
    seconds: u32,
    fps: u32,
    capture: String,
    tracks: Vec<String>,
    out: Option<PathBuf>,
) -> Result<()> {
    let base = out.unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Videos/Redline")
    });
    let config = ReplayConfig {
        capture,
        fps,
        replay_seconds: seconds,
        container: "mkv".into(),
        codec: None,
        replay_dir: base.join("Replay"),
        recording_dir: base.join("Recordings"),
        audio: tracks
            .iter()
            .map(|t| AudioTrack(t.split('|').map(str::to_owned).collect()))
            .collect(),
    };

    let launcher = Launcher::detect()?;
    eprintln!("Starting gsr ({launcher:?}), buffer {seconds}s, audio tracks: {tracks:?}");
    let mut session = ReplaySession::start(launcher, &config)?;
    eprintln!("Commands: [s]+Enter save replay, [r]+Enter start/stop recording, [q]+Enter quit");

    let (tx, keys) = mpsc::channel();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line.trim().to_owned()).is_err() {
                break;
            }
        }
    });

    loop {
        while let Ok(path) = session.saved.try_recv() {
            println!("saved: {}", path.display());
            let _ = tracks_summary(&path);
        }
        if !session.is_running() {
            eprintln!("gsr has exited");
            return Ok(());
        }
        match keys.recv_timeout(Duration::from_millis(200)) {
            Ok(k) if k == "s" => session.save_replay()?,
            Ok(k) if k == "r" => session.toggle_recording()?,
            Ok(k) if k == "q" => break,
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            // stdin closed (EOF, or no terminal): nobody can send commands any more,
            // so stop gsr instead of spinning until it exits by itself.
            Err(RecvTimeoutError::Disconnected) => {
                eprintln!("stdin closed: stopping gsr");
                break;
            }
        }
    }
    session.stop()?;
    // gsr may write the last path while it shuts down
    while let Ok(path) = session.saved.recv_timeout(Duration::from_millis(500)) {
        println!("saved: {}", path.display());
    }
    Ok(())
}

fn tracks(file: &Path) -> Result<()> {
    tracks_summary(file)
}

fn tracks_summary(file: &Path) -> Result<()> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=index,codec_type,codec_name,channels",
        ])
        .args(["-of", "compact=p=0"])
        .arg(file)
        .output()?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        println!("  {line}");
    }
    Ok(())
}
