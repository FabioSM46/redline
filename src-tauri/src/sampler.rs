//! Background GPU sampler: reads every AMD GPU once per second and keeps the last
//! 15 minutes of readings per GPU id, so the history survives while the window is hidden
//! in the tray (D8). The sysfs reading is in `redline-core::sensors`, the ring buffer in
//! `redline-core::history`; this module owns the thread and the shared state.

use redline_core::history::{History, Sample};
use redline_core::sensors::{AmdGpu, GpuSnapshot};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{App, Manager, State};

/// 1 Hz.
const PERIOD: Duration = Duration::from_secs(1);

/// The only windows `gpu_history` accepts, in seconds.
const WINDOWS_S: [u64; 3] = [60, 300, 900];

struct Entry {
    history: History,
    /// False once the GPU is no longer discovered: its history stays readable, but it
    /// has no current reading.
    present: bool,
}

/// One history per GPU id behind a single lock. The lock is never held while reading sysfs.
#[derive(Default)]
pub struct Sampler {
    gpus: Mutex<HashMap<String, Entry>>,
}

impl Sampler {
    /// One sampling round over the GPUs discovered right now. A GPU missing from `gpus`
    /// stops being sampled and is marked absent; a new one gets its own history.
    pub fn tick(&self, gpus: &[AmdGpu], now_ms: u64) {
        // Read sysfs first, without the lock. A card that vanished between discovery and
        // reading would yield an all-`None` snapshot: skip it instead of recording a hole.
        let readings: Vec<(String, GpuSnapshot)> = gpus
            .iter()
            .filter_map(|gpu| {
                let snapshot = gpu.snapshot();
                gpu.device
                    .exists()
                    .then(|| (gpu.pci_slot.clone(), snapshot))
            })
            .collect();

        let mut map = self.lock();
        for entry in map.values_mut() {
            entry.present = false;
        }
        for (id, snapshot) in readings {
            let entry = map.entry(id).or_insert_with(|| Entry {
                history: History::new(),
                present: true,
            });
            entry.present = true;
            entry.history.push(now_ms, snapshot);
        }
    }

    /// The most recent reading of the GPU with id `id`. A GPU the sampler has not seen yet
    /// (it appeared since the last round, or discovery has been failing) is read live with
    /// `live`, so this never disagrees with `list_gpus`; `live` names the id when the GPU
    /// does not exist. The lock is released before `live` runs.
    pub fn latest(
        &self,
        id: &str,
        live: impl FnOnce(&str) -> Result<GpuSnapshot, String>,
    ) -> Result<GpuSnapshot, String> {
        {
            let map = self.lock();
            if let Some(entry) = map.get(id) {
                if !entry.present {
                    return Err(format!("the GPU with id {id} is no longer present"));
                }
                if let Some(sample) = entry.history.latest() {
                    return Ok(sample.snapshot.clone());
                }
            }
        }
        live(id)
    }

    /// The readings of the last `window_s` seconds (60, 300 or 900), oldest first. A GPU
    /// that disappeared still returns what was recorded before it went away. A GPU the
    /// sampler has not seen yet has an empty history if `live` can read it, and is an
    /// error naming the id otherwise. The lock is released before `live` runs.
    pub fn history(
        &self,
        id: &str,
        window_s: u64,
        live: impl FnOnce(&str) -> Result<GpuSnapshot, String>,
    ) -> Result<Vec<Sample>, String> {
        if !WINDOWS_S.contains(&window_s) {
            return Err(format!("window_s must be 60, 300 or 900, got {window_s}"));
        }
        {
            let map = self.lock();
            if let Some(entry) = map.get(id) {
                return Ok(entry.history.window(window_s));
            }
        }
        live(id).map(|_| Vec::new())
    }

    /// A panic elsewhere must not take the history down with it: a poisoned map is
    /// still a consistent map, because every write is a single push.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.gpus.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A live sysfs read of one GPU, for the ids the sampler has not recorded yet.
pub fn read_live(id: &str) -> Result<GpuSnapshot, String> {
    AmdGpu::by_id(id)
        .map(|gpu| gpu.snapshot())
        .map_err(|e| e.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One round on the real sysfs. A failed discovery skips the round: the next one retries.
fn sample_once(sampler: &Sampler) -> Result<(), String> {
    let gpus = AmdGpu::discover().map_err(|e| format!("{e:#}"))?;
    sampler.tick(&gpus, now_ms());
    Ok(())
}

/// Logs a discovery failure once when it starts and once when it ends, not every second:
/// without it a sampler that records nothing would leave no trace of why.
fn log_transition(failing: &mut bool, result: Result<(), String>) {
    match result {
        Err(error) if !*failing => {
            *failing = true;
            eprintln!("redline: GPU sampler cannot discover GPUs, retrying every second: {error}");
        }
        Ok(()) if *failing => {
            *failing = false;
            eprintln!("redline: GPU sampler discovers GPUs again");
        }
        _ => {}
    }
}

/// Registers the sampler as Tauri state and starts its thread. The first round runs
/// before returning, so `gpu_snapshot` has a reading as soon as the window opens.
pub fn start(app: &mut App) -> std::io::Result<()> {
    let sampler = Sampler::default();
    let mut failing = false;
    log_transition(&mut failing, sample_once(&sampler));
    app.manage(sampler);

    let handle = app.handle().clone();
    thread::Builder::new()
        .name("gpu-sampler".into())
        .spawn(move || {
            let mut next = Instant::now() + PERIOD;
            loop {
                thread::sleep(next.saturating_duration_since(Instant::now()));
                // A late round (suspend, heavy load) restarts the schedule instead of
                // firing a burst of catch-up rounds.
                next = (next + PERIOD).max(Instant::now());
                log_transition(&mut failing, sample_once(&handle.state::<Sampler>()));
            }
        })?;
    Ok(())
}

/// The readings of the last `window_s` seconds (60, 300 or 900) of the GPU with id
/// `gpu_id`, oldest first. Any other window, or an unknown id, is an error.
#[tauri::command]
pub fn gpu_history(
    gpu_id: String,
    window_s: u64,
    sampler: State<'_, Sampler>,
) -> Result<Vec<Sample>, String> {
    sampler.history(&gpu_id, window_s, read_live)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    /// Fake sysfs tree: `<root>/drm/cardN/device -> <root>/devices/<slot>`, as in the
    /// kernel, with an edge temperature of `temp_c` and nothing else.
    fn fake_card(root: &Path, card: &str, slot: &str, temp_c: u64) {
        let device = root.join("devices").join(slot);
        let hwmon = device.join("hwmon/hwmon0");
        fs::create_dir_all(&hwmon).unwrap();
        fs::write(device.join("vendor"), "0x1002\n").unwrap();
        fs::write(hwmon.join("temp1_label"), "edge\n").unwrap();
        fs::write(hwmon.join("temp1_input"), format!("{}\n", temp_c * 1000)).unwrap();
        let card_dir = root.join("drm").join(card);
        fs::create_dir_all(&card_dir).unwrap();
        symlink(&device, card_dir.join("device")).unwrap();
    }

    fn remove_card(root: &Path, card: &str, slot: &str) {
        fs::remove_dir_all(root.join("drm").join(card)).unwrap();
        fs::remove_dir_all(root.join("devices").join(slot)).unwrap();
    }

    /// Removed when the test ends, also when it fails.
    fn temp_root(name: &str) -> TempDir {
        let root = TempDir::new("sampler", name);
        fs::create_dir_all(root.join("drm")).unwrap();
        root
    }

    fn tick_at(sampler: &Sampler, root: &Path, now_ms: u64) {
        let gpus = AmdGpu::discover_in(&root.join("drm")).unwrap();
        sampler.tick(&gpus, now_ms);
    }

    /// `live` for a sysfs tree where the GPU does not exist.
    fn no_gpu(id: &str) -> Result<GpuSnapshot, String> {
        Err(format!("no AMD GPU with id {id}"))
    }

    /// `live` reading the fake sysfs tree under `root`, like `read_live` on the real one.
    fn live_in(root: &Path) -> impl Fn(&str) -> Result<GpuSnapshot, String> + '_ {
        move |id| {
            AmdGpu::by_id_in(&root.join("drm"), id)
                .map(|gpu| gpu.snapshot())
                .map_err(|e| e.to_string())
        }
    }

    const A: &str = "0000:06:00.0";
    const B: &str = "0000:0d:00.0";

    #[test]
    fn samples_every_gpu_into_its_own_history() {
        let root = temp_root("two");
        fake_card(&root, "card0", A, 40);
        fake_card(&root, "card1", B, 60);
        let sampler = Sampler::default();
        for i in 0..3 {
            tick_at(&sampler, &root, 10_000 + i * 1000);
        }

        assert_eq!(sampler.latest(A, no_gpu).unwrap().temp_edge_c, Some(40.0));
        assert_eq!(sampler.latest(B, no_gpu).unwrap().temp_edge_c, Some(60.0));
        let h = sampler.history(B, 60, no_gpu).unwrap();
        let ts: Vec<u64> = h.iter().map(|s| s.timestamp_ms).collect();
        assert_eq!(ts, vec![10_000, 11_000, 12_000]);
    }

    #[test]
    fn latest_follows_the_newest_reading() {
        let root = temp_root("latest");
        fake_card(&root, "card0", A, 40);
        let sampler = Sampler::default();
        tick_at(&sampler, &root, 1_000);
        fs::write(
            root.join("devices")
                .join(A)
                .join("hwmon/hwmon0/temp1_input"),
            "55000\n",
        )
        .unwrap();
        tick_at(&sampler, &root, 2_000);
        assert_eq!(sampler.latest(A, no_gpu).unwrap().temp_edge_c, Some(55.0));
    }

    #[test]
    fn missing_sensors_stay_null_in_the_history() {
        let root = temp_root("null");
        fake_card(&root, "card0", A, 40);
        let sampler = Sampler::default();
        tick_at(&sampler, &root, 1_000);

        let json = serde_json::to_value(sampler.history(A, 60, no_gpu).unwrap()).unwrap();
        assert_eq!(json[0]["snapshot"]["temp_edge_c"], 40.0);
        assert!(json[0]["snapshot"]["power_w"].is_null());
        assert!(json[0]["snapshot"]["fan_rpm"].is_null());
    }

    #[test]
    fn unknown_id_is_an_error_that_names_it() {
        let root = temp_root("unknown");
        let sampler = Sampler::default();
        let err = sampler.latest("0000:ff:00.0", live_in(&root)).unwrap_err();
        assert!(err.contains("0000:ff:00.0"), "{err}");
        let err = sampler
            .history("0000:ff:00.0", 60, live_in(&root))
            .unwrap_err();
        assert!(err.contains("0000:ff:00.0"), "{err}");
    }

    #[test]
    fn a_gpu_not_sampled_yet_is_read_live() {
        let root = temp_root("live");
        fake_card(&root, "card0", A, 40);
        let sampler = Sampler::default();
        tick_at(&sampler, &root, 1_000);
        // appears between two rounds: list_gpus already returns it
        fake_card(&root, "card1", B, 70);

        let snapshot = sampler.latest(B, live_in(&root)).unwrap();
        assert_eq!(snapshot.temp_edge_c, Some(70.0));
        assert!(sampler.history(B, 60, live_in(&root)).unwrap().is_empty());
        // a sampled GPU is served from its history, not read again
        fs::write(
            root.join("devices")
                .join(A)
                .join("hwmon/hwmon0/temp1_input"),
            "99000\n",
        )
        .unwrap();
        assert_eq!(
            sampler.latest(A, live_in(&root)).unwrap().temp_edge_c,
            Some(40.0)
        );
    }

    #[test]
    fn discovery_failures_are_logged_only_on_transitions() {
        let mut failing = false;
        log_transition(&mut failing, Ok(()));
        assert!(!failing);
        log_transition(&mut failing, Err("boom".into()));
        assert!(failing);
        log_transition(&mut failing, Err("boom".into()));
        assert!(failing);
        log_transition(&mut failing, Ok(()));
        assert!(!failing);
    }

    #[test]
    fn only_the_three_windows_are_accepted() {
        let root = temp_root("windows");
        fake_card(&root, "card0", A, 40);
        let sampler = Sampler::default();
        tick_at(&sampler, &root, 1_000);

        for w in WINDOWS_S {
            assert_eq!(sampler.history(A, w, no_gpu).unwrap().len(), 1);
        }
        for w in [0, 1, 59, 61, 120, 901, 3600, u64::MAX] {
            let err = sampler.history(A, w, no_gpu).unwrap_err();
            assert!(err.contains("window_s"), "{w}: {err}");
        }
    }

    #[test]
    fn a_gpu_that_disappears_stops_being_sampled() {
        let root = temp_root("gone");
        fake_card(&root, "card0", A, 40);
        fake_card(&root, "card1", B, 60);
        let sampler = Sampler::default();
        tick_at(&sampler, &root, 1_000);

        remove_card(&root, "card1", B);
        tick_at(&sampler, &root, 2_000);
        tick_at(&sampler, &root, 3_000);

        assert_eq!(sampler.history(A, 60, no_gpu).unwrap().len(), 3);
        let err = sampler.latest(B, no_gpu).unwrap_err();
        assert!(err.contains(B), "{err}");
        let kept = sampler.history(B, 60, no_gpu).unwrap();
        assert_eq!(kept.len(), 1, "no new samples after it disappeared");
        assert_eq!(kept[0].timestamp_ms, 1_000);

        // and it is sampled again if it comes back
        fake_card(&root, "card1", B, 61);
        tick_at(&sampler, &root, 4_000);
        assert_eq!(sampler.latest(B, no_gpu).unwrap().temp_edge_c, Some(61.0));
    }

    #[test]
    fn a_card_removed_after_discovery_is_skipped() {
        let root = temp_root("race");
        fake_card(&root, "card0", A, 40);
        let gpus = AmdGpu::discover_in(&root.join("drm")).unwrap();
        remove_card(&root, "card0", A);

        let sampler = Sampler::default();
        sampler.tick(&gpus, 1_000);
        assert!(sampler.latest(A, no_gpu).is_err());
    }
}
