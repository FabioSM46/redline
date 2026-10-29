//! The tuning service: what LACT can change on a GPU, and why not when it cannot.
//!
//! Read-only for now: it detects LACT (installed, `lactd` running, socket permission, version)
//! and reads one GPU's tunable state through `redline-core::lact`. Every request goes to the
//! `lactd` socket from a blocking thread, never from the main thread. Redline never writes to
//! sysfs and never runs as root: the setters that come later go through `lactd` with the user's
//! confirmation (AGENTS.md, "GPU Rules").
//!
//! A missing tool is a disabled feature, not an error: every failure becomes
//! `available: false` with a sentence that says what to do, and a GPU LACT does not manage is
//! "not tunable", not an error. A LACT version other than [`VERIFIED_VERSION`] fills `warning` and
//! blocks nothing. Values LACT does not report are `null`, never zero.

use redline_core::lact::{
    self, DeviceFlag, DeviceStats, FanControlMode, FanInfo, LactClient, LactError, PmfwInfo,
    VERIFIED_VERSION,
};
use redline_core::thumbnail::find_program;
use serde::Serialize;
use std::ffi::OsString;
use std::sync::Arc;
use tauri::{App, Manager, State};

/// The binary every LACT package installs (GUI, CLI and the daemon itself).
const LACT_BINARY: &str = "lact";

pub const NOT_INSTALLED: &str = "LACT is not installed. Install LACT 0.10.1 from its GitHub \
    releases or your distribution's packages, then start its daemon with \
    `sudo systemctl enable --now lactd` to tune the GPU.";
pub const NOT_RUNNING: &str = "LACT is installed but its daemon is not running. Start it with \
    `sudo systemctl enable --now lactd`.";
pub const NO_PERMISSION: &str = "Redline cannot talk to the LACT daemon: your user is not in its \
    admin group. Set `admin_group` in /etc/lact/config.yaml to a group you belong to, restart it \
    with `sudo systemctl restart lactd`, and log out and back in if you just joined the group.";
pub const NOT_MANAGED: &str = "LACT does not manage this GPU, so it cannot be tuned.";

/// What `tuning_status` returns. `power` and `fan` are `null` when `available` is false.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TuningStatus {
    pub available: bool,
    /// Why tuning is not available, as a sentence that says what to do.
    pub unavailable_reason: Option<String>,
    /// A LACT version other than the verified one: shown, never a block.
    pub warning: Option<String>,
    /// The version `lactd` reports.
    pub lact_version: Option<String>,
    /// The `amdgpu` overdrive bit, as LACT reads it; `null` when unknown.
    pub overdrive_enabled: Option<bool>,
    pub power: Option<PowerLimit>,
    pub fan: Option<FanStatus>,
    /// The active LACT profile; `null` is LACT's default profile (or an unreadable list).
    pub profile: Option<String>,
}

/// The power cap, in watts.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PowerLimit {
    pub current_w: Option<f64>,
    pub min_w: Option<f64>,
    pub max_w: Option<f64>,
    pub default_w: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FanMode {
    Static,
    Curve,
}

/// One point of a fan curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CurvePoint {
    pub temperature_c: i32,
    pub speed_pct: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TemperatureRange {
    pub min_c: i32,
    pub max_c: i32,
}

/// The fan as LACT sees it. `mode`, `static_speed` and `curve` are LACT's stored settings,
/// `null` until the fan has been configured once.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FanStatus {
    /// LACT can drive this fan. On RDNA3/4 it needs overdrive (the firmware fan curve).
    pub configurable: bool,
    /// LACT drives the fan now; false = the firmware's automatic control.
    pub control_enabled: bool,
    pub mode: Option<FanMode>,
    /// Percent, 0–100.
    pub static_speed: Option<f64>,
    /// Sorted by temperature; speeds in percent.
    pub curve: Option<Vec<CurvePoint>>,
    /// The temperatures a firmware (PMFW) curve point may use.
    pub temperature_range: Option<TemperatureRange>,
    pub pmfw: PmfwStatus,
}

/// A firmware fan value and the range it may be set to (`min`/`max` are `null` when the
/// firmware does not allow changing it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PmfwValue {
    pub current: u32,
    pub min: Option<u32>,
    pub max: Option<u32>,
}

/// The RDNA3+ firmware fan options, named as the fan setter takes them. Each is `null` when the
/// GPU does not expose it (older GPUs, or RDNA3/4 with overdrive off).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PmfwStatus {
    /// RPM.
    pub acoustic_limit: Option<PmfwValue>,
    /// RPM.
    pub acoustic_target: Option<PmfwValue>,
    /// °C.
    pub target_temperature: Option<PmfwValue>,
    /// Percent.
    pub minimum_pwm: Option<PmfwValue>,
    pub zero_rpm: Option<bool>,
    /// °C.
    pub zero_rpm_threshold: Option<PmfwValue>,
}

impl TuningStatus {
    fn unavailable(reason: impl Into<String>) -> Self {
        TuningStatus {
            available: false,
            unavailable_reason: Some(reason.into()),
            warning: None,
            lact_version: None,
            overdrive_enabled: None,
            power: None,
            fan: None,
            profile: None,
        }
    }
}

/// `None` for the verified version, otherwise the warning sentence.
pub fn version_notice(version: &str) -> Option<String> {
    (version != VERIFIED_VERSION).then(|| {
        format!(
            "LACT {version} is running, but Redline is verified with LACT {VERIFIED_VERSION}: \
             some tuning features may not work."
        )
    })
}

/// Where LACT is reached and where it is looked for. Tests pass a fake socket and PATH.
pub struct Lact {
    client: LactClient,
    search_path: OsString,
}

impl Lact {
    pub fn new(client: LactClient, search_path: OsString) -> Self {
        Lact {
            client,
            search_path,
        }
    }

    /// The system's `lactd` socket and the process PATH.
    pub fn system() -> Self {
        Lact::new(
            LactClient::default(),
            std::env::var_os("PATH").unwrap_or_default(),
        )
    }

    fn installed(&self) -> bool {
        find_program(LACT_BINARY, &self.search_path).is_some()
    }

    /// The sentence for a failed request.
    pub fn reason(&self, error: &LactError) -> String {
        match error {
            LactError::SocketMissing | LactError::NotRunning if self.installed() => {
                NOT_RUNNING.to_owned()
            }
            LactError::SocketMissing | LactError::NotRunning => NOT_INSTALLED.to_owned(),
            LactError::PermissionDenied => NO_PERMISSION.to_owned(),
            other => format!(
                "The LACT daemon did not answer as expected ({other}). Restarting it with \
                 `sudo systemctl restart lactd` may help."
            ),
        }
    }

    /// The tuning state of the GPU with Redline id `gpu_id` (its PCI slot). Blocking: up to
    /// five requests to `lactd`, each bounded by the client's timeout.
    pub fn status(&self, gpu_id: &str) -> TuningStatus {
        let system = match self.client.system_info() {
            Ok(system) => system,
            Err(error) => return TuningStatus::unavailable(self.reason(&error)),
        };
        let mut status = TuningStatus {
            warning: version_notice(&system.version),
            lact_version: Some(system.version),
            overdrive_enabled: system.amdgpu_overdrive_enabled,
            ..TuningStatus::unavailable(NOT_MANAGED)
        };

        let devices = match self.client.list_devices() {
            Ok(devices) => devices,
            Err(error) => {
                status.unavailable_reason = Some(self.reason(&error));
                return status;
            }
        };
        let Some(device) = lact::find_device(&devices, gpu_id) else {
            return status;
        };
        let read = self
            .client
            .device_info(&device.id)
            .and_then(|info| Ok((info, self.client.device_stats(&device.id)?)));
        let (info, stats) = match read {
            Ok(read) => read,
            Err(error) => {
                status.unavailable_reason = Some(self.reason(&error));
                return status;
            }
        };

        status.available = true;
        status.unavailable_reason = None;
        status.power = Some(power_limit(&stats));
        status.fan = Some(fan_status(
            info.has(&DeviceFlag::ConfigurableFanControl),
            &stats,
        ));
        status.profile = self
            .client
            .list_profiles()
            .ok()
            .and_then(|p| p.current_profile);
        status
    }
}

fn power_limit(stats: &DeviceStats) -> PowerLimit {
    let power = &stats.power;
    PowerLimit {
        current_w: power.cap_current,
        min_w: power.cap_min,
        max_w: power.cap_max,
        default_w: power.cap_default,
    }
}

/// 0.0–1.0 → percent, to one decimal: `0.35` is `35.0`, not `35.000004`.
fn percent(fraction: f32) -> f64 {
    (f64::from(fraction) * 1000.0).round() / 10.0
}

fn fan_status(configurable: bool, stats: &DeviceStats) -> FanStatus {
    let fan = &stats.fan;
    FanStatus {
        configurable,
        control_enabled: fan.control_enabled,
        // A mode a later LACT adds is unknown here: `null`, not a failure.
        mode: fan.control_mode.as_ref().and_then(|mode| match mode {
            FanControlMode::Static => Some(FanMode::Static),
            FanControlMode::Curve => Some(FanMode::Curve),
            FanControlMode::Other(_) => None,
        }),
        static_speed: fan.static_speed.map(percent),
        curve: fan.curve.as_ref().map(|curve| {
            curve
                .iter()
                .map(|(&temperature_c, &speed)| CurvePoint {
                    temperature_c,
                    speed_pct: percent(speed),
                })
                .collect()
        }),
        temperature_range: fan
            .temperature_range
            .map(|(min_c, max_c)| TemperatureRange { min_c, max_c }),
        pmfw: pmfw_status(&fan.pmfw_info),
    }
}

fn pmfw_value(info: Option<FanInfo>) -> Option<PmfwValue> {
    info.map(|info| PmfwValue {
        current: info.current,
        min: info.allowed_range.map(|(min, _)| min),
        max: info.allowed_range.map(|(_, max)| max),
    })
}

fn pmfw_status(pmfw: &PmfwInfo) -> PmfwStatus {
    PmfwStatus {
        acoustic_limit: pmfw_value(pmfw.acoustic_limit),
        acoustic_target: pmfw_value(pmfw.acoustic_target),
        target_temperature: pmfw_value(pmfw.target_temp),
        minimum_pwm: pmfw_value(pmfw.minimum_pwm),
        zero_rpm: pmfw.zero_rpm_enable,
        zero_rpm_threshold: pmfw_value(pmfw.zero_rpm_temperature),
    }
}

/// The tuning service, shared as Tauri state. Cheap to clone; the later setters and the
/// pending-change mechanism keep their state in here.
#[derive(Clone)]
pub struct Tuning {
    lact: Arc<Lact>,
}

impl Tuning {
    pub fn new(lact: Lact) -> Self {
        Tuning {
            lact: Arc::new(lact),
        }
    }

    pub fn lact(&self) -> &Lact {
        &self.lact
    }
}

/// Registers the service. Nothing is contacted until the first command.
pub fn init(app: &mut App) {
    app.manage(Tuning::new(Lact::system()));
}

/// The tuning state of the GPU with id `gpu_id` (its PCI slot). Never an error for a missing
/// LACT or an unmanaged GPU: those are `available: false` with the reason.
#[tauri::command]
pub async fn tuning_status(
    gpu_id: String,
    tuning: State<'_, Tuning>,
) -> Result<TuningStatus, String> {
    let tuning = tuning.inner().clone();
    tauri::async_runtime::spawn_blocking(move || tuning.lact().status(&gpu_id))
        .await
        .map_err(|error| format!("The tuning service did not answer: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::{Value, json};
    use std::fs;
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(2);
    const GPU_A: &str = "1002:164e-1043:8877-0000:0c:00.0";
    const GPU_B: &str = "1002:7550-1043:0643-0000:0d:00.0";

    type Handler = dyn Fn(&str, &Value) -> Value + Send + Sync;

    /// A fake `lactd`: answers each request line with `ok` and the handler's data, or with
    /// `error` when the handler returns `{"error": "<message>"}`. Stopped (and joined) on drop.
    struct FakeLactd {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl FakeLactd {
        fn start(socket: &Path, handler: Box<Handler>) -> Self {
            let listener = UnixListener::bind(socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let thread = thread::spawn(move || {
                while !stopping.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
                            let mut line = String::new();
                            if BufReader::new(&stream).read_line(&mut line).is_err() {
                                continue;
                            }
                            let request: Value = serde_json::from_str(&line).unwrap();
                            let command = request["command"].as_str().unwrap_or_default();
                            let data = handler(command, &request["args"]);
                            let answer = match data.get("error") {
                                Some(message) => json!({
                                    "status": "error",
                                    "data": { "description": message, "source": null }
                                }),
                                None => json!({ "status": "ok", "data": data }),
                            };
                            let _ = writeln!(&stream, "{answer}");
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            FakeLactd {
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for FakeLactd {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn stats_for(cap: f64) -> Value {
        json!({
            "fan": {
                "control_enabled": false, "pwm_current": 0, "speed_current": 0,
                "speed_max": 6000, "pwm_max": 255, "pmfw_info": {}
            },
            "power": {
                "average": 21.0, "cap_current": cap, "cap_max": 340.0,
                "cap_min": 221.0, "cap_default": 317.0
            },
            "performance_level": "auto"
        })
    }

    /// lactd 0.10.1 with two GPUs (the second one is the RX 9070 XT the shapes were taken from).
    fn two_gpus(version: &'static str) -> Box<Handler> {
        Box::new(move |command, args| match command {
            "system_info" => json!({
                "version": version, "commit": "30dc182", "profile": "release",
                "kernel_version": "6.8.0", "amdgpu_overdrive_enabled": false
            }),
            "list_devices" => json!([
                { "id": GPU_A, "name": "iGPU", "device_type": "Integrated" },
                { "id": GPU_B, "name": "dGPU", "device_type": "Dedicated" }
            ]),
            "device_info" => {
                assert_eq!(args["include_api_info"], false);
                json!({ "driver": "amdgpu", "flags": ["DumpableVBios", "HasPmfw"] })
            }
            "device_stats" if args["id"] == GPU_A => stats_for(15.0),
            "device_stats" if args["id"] == GPU_B => stats_for(317.0),
            "list_profiles" => {
                json!({ "profiles": {}, "current_profile": null, "auto_switch": false })
            }
            _ => json!({ "error": format!("Controller '{}' not found", args["id"]) }),
        })
    }

    struct Setup {
        dir: TempDir,
    }

    impl Setup {
        fn new(name: &str) -> Self {
            Setup {
                dir: TempDir::new("tuning", name),
            }
        }
        fn socket(&self) -> PathBuf {
            self.dir.join("lactd.sock")
        }
        /// A PATH with an executable `lact` in it, or an empty one.
        fn lact(&self, installed: bool) -> Lact {
            let bin = self.dir.join("bin");
            fs::create_dir_all(&bin).unwrap();
            if installed {
                let lact = bin.join(LACT_BINARY);
                fs::write(&lact, "#!/bin/sh\n").unwrap();
                fs::set_permissions(&lact, fs::Permissions::from_mode(0o755)).unwrap();
            }
            Lact::new(
                LactClient::new(self.socket()).with_timeout(TIMEOUT),
                bin.into_os_string(),
            )
        }
    }

    #[test]
    fn without_lact_says_to_install_it() {
        let setup = Setup::new("missing");
        let status = setup.lact(false).status("0000:0d:00.0");
        assert_eq!(status, TuningStatus::unavailable(NOT_INSTALLED));
    }

    #[test]
    fn installed_without_a_daemon_says_to_start_it() {
        let setup = Setup::new("stopped");
        let lact = setup.lact(true);
        assert_eq!(
            lact.status("0000:0d:00.0").unavailable_reason.as_deref(),
            Some(NOT_RUNNING)
        );
        // A socket left behind by a stopped daemon reads the same.
        drop(UnixListener::bind(setup.socket()).unwrap());
        assert_eq!(
            lact.status("0000:0d:00.0").unavailable_reason.as_deref(),
            Some(NOT_RUNNING)
        );
    }

    #[test]
    fn a_socket_the_user_may_not_open_names_the_admin_group() {
        // root opens any socket: the check means nothing there.
        if fs::metadata("/proc/self").is_ok_and(|meta| meta.uid() == 0) {
            return;
        }
        let setup = Setup::new("permission");
        let _lactd = FakeLactd::start(&setup.socket(), two_gpus(VERIFIED_VERSION));
        fs::set_permissions(setup.socket(), fs::Permissions::from_mode(0o000)).unwrap();
        let status = setup.lact(true).status("0000:0d:00.0");
        assert_eq!(status.unavailable_reason.as_deref(), Some(NO_PERMISSION));
        assert!(!status.available);
    }

    #[test]
    fn maps_each_redline_id_to_its_own_lact_device() {
        let setup = Setup::new("two");
        let _lactd = FakeLactd::start(&setup.socket(), two_gpus(VERIFIED_VERSION));
        let lact = setup.lact(true);

        let b = lact.status("0000:0d:00.0");
        assert!(b.available, "{b:?}");
        assert_eq!(b.warning, None);
        assert_eq!(b.lact_version.as_deref(), Some(VERIFIED_VERSION));
        assert_eq!(b.overdrive_enabled, Some(false));
        assert_eq!(
            b.power,
            Some(PowerLimit {
                current_w: Some(317.0),
                min_w: Some(221.0),
                max_w: Some(340.0),
                default_w: Some(317.0),
            })
        );
        let a = lact.status("0000:0c:00.0");
        assert_eq!(a.power.unwrap().current_w, Some(15.0));
    }

    #[test]
    fn a_gpu_lact_does_not_manage_is_not_tunable_and_not_an_error() {
        let setup = Setup::new("unknown");
        let _lactd = FakeLactd::start(&setup.socket(), two_gpus(VERIFIED_VERSION));
        let status = setup.lact(true).status("0000:03:00.0");
        assert!(!status.available);
        assert_eq!(status.unavailable_reason.as_deref(), Some(NOT_MANAGED));
        // What LACT said about the system is still there.
        assert_eq!(status.overdrive_enabled, Some(false));
        assert_eq!(status.power, None);
    }

    #[test]
    fn another_lact_version_warns_without_blocking() {
        let setup = Setup::new("version");
        let _lactd = FakeLactd::start(&setup.socket(), two_gpus("0.11.0"));
        let status = setup.lact(true).status("0000:0d:00.0");
        assert!(status.available);
        let warning = status.warning.unwrap();
        assert!(warning.contains("0.11.0") && warning.contains(VERIFIED_VERSION));
    }

    #[test]
    fn what_lact_does_not_report_is_null_never_zero() {
        let setup = Setup::new("nulls");
        let _lactd = FakeLactd::start(
            &setup.socket(),
            Box::new(|command, _| match command {
                "system_info" => json!({ "version": VERIFIED_VERSION }),
                // Values a later LACT might add: still readable, unknown ones are `null`.
                "list_devices" => json!([{ "id": GPU_B, "device_type": "Virtual" }]),
                "device_info" => json!({}),
                "device_stats" => json!({ "fan": { "control_mode": "auto" }, "power": {} }),
                _ => json!({ "error": "no profiles" }),
            }),
        );
        let status = setup.lact(true).status("0000:0d:00.0");
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["available"], true);
        assert_eq!(json["overdrive_enabled"], Value::Null);
        assert_eq!(json["profile"], Value::Null);
        for key in ["current_w", "min_w", "max_w", "default_w"] {
            assert_eq!(json["power"][key], Value::Null, "{key}");
        }
        let fan = &json["fan"];
        assert_eq!(fan["configurable"], false);
        for key in ["mode", "static_speed", "curve", "temperature_range"] {
            assert_eq!(fan[key], Value::Null, "{key}");
        }
        for (key, value) in fan["pmfw"].as_object().unwrap() {
            assert_eq!(*value, Value::Null, "{key}");
        }
    }

    #[test]
    fn reads_a_configured_fan_and_the_firmware_options() {
        let setup = Setup::new("fan");
        let _lactd = FakeLactd::start(
            &setup.socket(),
            Box::new(|command, _| match command {
                "system_info" => {
                    json!({ "version": VERIFIED_VERSION, "amdgpu_overdrive_enabled": true })
                }
                "list_devices" => json!([{ "id": GPU_B }]),
                "device_info" => json!({ "flags": ["HasPmfw", "ConfigurableFanControl"] }),
                "device_stats" => json!({
                    "fan": {
                        "control_enabled": true, "control_mode": "curve", "static_speed": 0.35,
                        "curve": { "70": 0.75, "40": 0.3 }, "temperature_range": [25, 100],
                        "pmfw_info": {
                            "acoustic_limit": { "current": 2450, "allowed_range": [500, 3200] },
                            "minimum_pwm": { "current": 15, "allowed_range": null },
                            "zero_rpm_enable": true
                        }
                    },
                    "power": {}
                }),
                "list_profiles" => json!({ "current_profile": "Quiet", "auto_switch": false }),
                _ => json!({ "error": "unexpected" }),
            }),
        );
        let status = setup.lact(true).status("0000:0d:00.0");
        assert_eq!(status.overdrive_enabled, Some(true));
        assert_eq!(status.profile.as_deref(), Some("Quiet"));
        let fan = status.fan.unwrap();
        assert!(fan.configurable && fan.control_enabled);
        assert_eq!(fan.mode, Some(FanMode::Curve));
        assert_eq!(fan.static_speed, Some(35.0));
        assert_eq!(
            fan.curve.unwrap(),
            vec![
                CurvePoint {
                    temperature_c: 40,
                    speed_pct: 30.0
                },
                CurvePoint {
                    temperature_c: 70,
                    speed_pct: 75.0
                },
            ]
        );
        assert_eq!(
            fan.temperature_range,
            Some(TemperatureRange {
                min_c: 25,
                max_c: 100
            })
        );
        assert_eq!(
            fan.pmfw.acoustic_limit,
            Some(PmfwValue {
                current: 2450,
                min: Some(500),
                max: Some(3200)
            })
        );
        assert_eq!(
            fan.pmfw.minimum_pwm,
            Some(PmfwValue {
                current: 15,
                min: None,
                max: None
            })
        );
        assert_eq!(fan.pmfw.zero_rpm, Some(true));
        assert_eq!(fan.pmfw.target_temperature, None);
    }

    #[test]
    fn a_daemon_error_is_a_sentence_not_a_failure() {
        let setup = Setup::new("daemon-error");
        let _lactd = FakeLactd::start(
            &setup.socket(),
            Box::new(|command, _| match command {
                "system_info" => json!({ "version": VERIFIED_VERSION }),
                "list_devices" => json!([{ "id": GPU_B }]),
                _ => json!({ "error": "Could not read the GPU" }),
            }),
        );
        let status = setup.lact(true).status("0000:0d:00.0");
        assert!(!status.available);
        let reason = status.unavailable_reason.unwrap();
        assert!(reason.contains("Could not read the GPU"), "{reason}");
        assert!(reason.contains("systemctl restart lactd"), "{reason}");
    }

    #[test]
    fn the_version_notice_is_empty_only_for_the_verified_version() {
        assert_eq!(version_notice(VERIFIED_VERSION), None);
        assert!(version_notice("0.10.0").is_some());
    }
}
