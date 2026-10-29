//! Client for the LACT daemon (`lactd`).
//!
//! Protocol: Unix socket, one JSON request per line, one JSON response per line.
//! Request:  `{"command": "<snake_case>", "args": {...}}`
//! Response: `{"status": "ok", "data": ...}` or `{"status": "error", "data": ...}`
//!
//! Verified against LACT [`VERIFIED_VERSION`] (`lact-schema/src/{request,response,lib,config}.rs`
//! and `lact-daemon/src/server/handler.rs` at the `v0.10.1` tag). The API is not a stable promise:
//! a different version is reported, never blocked, and every response type below is a
//! **read-only subset** of LACT's own: unknown fields are ignored and missing ones are `None`, so a
//! field LACT adds or drops does not break the whole answer.
//!
//! Commands with a typed answer, used by Redline's tuning (M3):
//!
//! | Command | Arguments | Answer |
//! |---|---|---|
//! | `ping` | — | — |
//! | `system_info` | — | [`SystemInfo`] |
//! | `list_devices` | — | `Vec<`[`DeviceListEntry`]`>` |
//! | `device_info` | `{ id, include_api_info: false }` | [`DeviceInfo`] |
//! | `device_stats` | `{ id }` | [`DeviceStats`] |
//! | `get_gpu_config` | `{ id }` | `Option<`[`GpuConfig`]`>` (`null` = never configured) |
//! | `list_profiles` | `{ include_state: false }` | [`ProfilesInfo`] |
//! | `set_power_cap` | `{ id, cap }` (`cap: null` = default) | seconds until lactd reverts |
//! | `set_fan_control` | [`FanOptions`] | seconds until lactd reverts |
//! | `confirm_pending_config` | `{ command: "confirm" \| "revert" }` | — |

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_SOCKET: &str = "/run/lactd.sock";
/// The LACT version every request and response shape in this module was checked against.
pub const VERIFIED_VERSION: &str = "0.10.1";
/// How long a request may wait for the socket, in each direction.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a request to `lactd` failed. The first three are about reaching the socket, and each has
/// a different fix; the others mean `lactd` was reached.
#[derive(Debug)]
pub enum LactError {
    /// No socket file: `lactd` is not running (or LACT is not installed).
    SocketMissing,
    /// The socket exists but nobody listens: `lactd` stopped and left it behind.
    NotRunning,
    /// The socket exists but the user may not open it: they are not in lactd's admin group.
    PermissionDenied,
    /// Any other failure while connecting, writing or reading (a timeout included).
    Io(io::Error),
    /// `lactd` answered something this client does not understand.
    Protocol(String),
    /// `lactd` understood the request and refused it; the message is its own.
    Daemon(String),
}

impl fmt::Display for LactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LactError::SocketMissing => write!(f, "the lactd socket does not exist"),
            LactError::NotRunning => write!(f, "nothing is listening on the lactd socket"),
            LactError::PermissionDenied => write!(f, "permission denied on the lactd socket"),
            LactError::Io(error) => write!(f, "talking to lactd failed: {error}"),
            LactError::Protocol(message) => write!(f, "unexpected answer from lactd: {message}"),
            LactError::Daemon(message) => write!(f, "lactd: {message}"),
        }
    }
}

impl std::error::Error for LactError {}

impl LactError {
    fn connecting(error: io::Error) -> Self {
        match error.kind() {
            ErrorKind::NotFound => LactError::SocketMissing,
            ErrorKind::ConnectionRefused => LactError::NotRunning,
            ErrorKind::PermissionDenied => LactError::PermissionDenied,
            _ => LactError::Io(error),
        }
    }
}

pub type LactResult<T> = Result<T, LactError>;

// ---- Responses (read-only subsets of lact-schema 0.10.1) ----

/// `system_info`. `SystemInfo` in lact-schema, with `VersionInfo` flattened into it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemInfo {
    /// The daemon's version, for example `0.10.1`.
    pub version: String,
    pub commit: Option<String>,
    pub kernel_version: Option<String>,
    pub distro: Option<String>,
    /// Whether the `amdgpu.ppfeaturemask` overdrive bit is set; `None` when LACT cannot tell.
    pub amdgpu_overdrive_enabled: Option<bool>,
    /// How LACT would enable overdrive on this system, as LACT sends it (for example
    /// `{"Modprobe": "Debian"}`). Kept untyped: only the overdrive issue reads it.
    pub amdgpu_params_configurator: Option<Value>,
}

/// An enum LACT sends as a string. A value a later LACT adds is kept by name in `Other` instead
/// of failing the whole answer: a new LACT is a warning, never an outage.
macro_rules! open_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal,)+ }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(from = "String", into = "String")]
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
            Other(String),
        }

        impl From<String> for $name {
            fn from(name: String) -> Self {
                match name.as_str() {
                    $($text => $name::$variant,)+
                    _ => $name::Other(name),
                }
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                match value {
                    $($name::$variant => $text.into(),)+
                    $name::Other(name) => name,
                }
            }
        }
    };
}

open_enum! {
    /// LACT's `DeviceType`.
    DeviceType {
        Dedicated = "Dedicated",
        Integrated = "Integrated",
    }
}

impl Default for DeviceType {
    /// LACT's own default when the field is missing.
    fn default() -> Self {
        DeviceType::Dedicated
    }
}

/// One entry of `list_devices`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceListEntry {
    /// `VENDOR:DEVICE-SUBVENDOR:SUBDEVICE-PCI_SLOT`, for example
    /// `1002:7550-1043:0643-0000:0d:00.0`.
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub device_type: DeviceType,
}

impl DeviceListEntry {
    /// The PCI slot at the end of the LACT id (Redline's GPU id), for example `0000:0d:00.0`.
    pub fn pci_slot(&self) -> &str {
        self.id.rsplit('-').next().unwrap_or(&self.id)
    }
}

/// The LACT device whose id ends with the PCI slot `pci_slot`, if LACT manages it.
pub fn find_device<'a>(
    devices: &'a [DeviceListEntry],
    pci_slot: &str,
) -> Option<&'a DeviceListEntry> {
    devices.iter().find(|device| device.pci_slot() == pci_slot)
}

open_enum! {
    /// LACT's `DeviceFlag`.
    DeviceFlag {
        /// LACT can drive the fan (on RDNA3+ only when the firmware fan curve is writable, which
        /// needs overdrive).
        ConfigurableFanControl = "ConfigurableFanControl",
        DumpableVBios = "DumpableVBios",
        /// RDNA3 and later: the fan is driven through the power management firmware.
        HasPmfw = "HasPmfw",
        AutoFanThreshold = "AutoFanThreshold",
    }
}

/// `device_info`, without the PCI, Vulkan, link and DRM details.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceInfo {
    pub driver: Option<String>,
    pub vbios_version: Option<String>,
    pub flags: Vec<DeviceFlag>,
}

impl DeviceInfo {
    pub fn has(&self, flag: &DeviceFlag) -> bool {
        self.flags.contains(flag)
    }
}

open_enum! {
    /// LACT's `FanControlMode`.
    FanControlMode {
        Static = "static",
        Curve = "curve",
    }
}

impl Default for FanControlMode {
    /// LACT's own default (`#[default] Curve`).
    fn default() -> Self {
        FanControlMode::Curve
    }
}

/// Temperature (°C) → fan speed (0.0–1.0), as LACT stores a curve (`FanCurveMap`). JSON object
/// keys are strings: serde_json parses them back into integers.
pub type FanCurveMap = BTreeMap<i32, f32>;

/// One PMFW fan value (`amdgpu_sysfs::FanInfo`): the current value and, when the firmware lets it
/// be changed, the allowed range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanInfo {
    pub current: u32,
    #[serde(default)]
    pub allowed_range: Option<(u32, u32)>,
}

/// `device_stats.fan.pmfw_info`: what the firmware fan control of RDNA3+ exposes. Empty
/// (every field `None`) on older GPUs, and on RDNA3/4 while overdrive is off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PmfwInfo {
    pub acoustic_limit: Option<FanInfo>,
    pub acoustic_target: Option<FanInfo>,
    pub target_temp: Option<FanInfo>,
    pub minimum_pwm: Option<FanInfo>,
    pub zero_rpm_enable: Option<bool>,
    pub zero_rpm_temperature: Option<FanInfo>,
}

/// `device_stats.fan`. `control_mode`, `static_speed` and `curve` come from LACT's stored
/// configuration and are `None` until the fan has been configured once.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FanStats {
    pub control_enabled: bool,
    pub control_mode: Option<FanControlMode>,
    /// 0.0–1.0.
    pub static_speed: Option<f32>,
    pub curve: Option<FanCurveMap>,
    pub pwm_current: Option<u8>,
    pub speed_current: Option<u32>,
    pub speed_max: Option<u32>,
    pub speed_min: Option<u32>,
    pub pwm_max: Option<u32>,
    pub pwm_min: Option<u32>,
    /// The temperatures a PMFW fan curve point may use, in °C.
    pub temperature_range: Option<(i32, i32)>,
    pub temperature_key: Option<String>,
    pub spindown_delay_ms: Option<u64>,
    pub change_threshold: Option<u64>,
    pub pmfw_info: PmfwInfo,
}

/// `device_stats.power`, in watts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerStats {
    pub average: Option<f64>,
    pub current: Option<f64>,
    pub cap_current: Option<f64>,
    pub cap_max: Option<f64>,
    pub cap_min: Option<f64>,
    pub cap_default: Option<f64>,
}

/// `device_stats`, only the parts tuning reads; temperatures, clocks and the rest come from
/// sysfs through `sensors`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceStats {
    pub fan: FanStats,
    pub power: PowerStats,
    pub performance_level: Option<String>,
}

/// `PmfwOptions`: the PMFW fan values stored in a GPU's configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PmfwOptions {
    pub acoustic_limit: Option<u32>,
    pub acoustic_target: Option<u32>,
    pub minimum_pwm: Option<u32>,
    pub target_temperature: Option<u32>,
    pub zero_rpm: Option<bool>,
    pub zero_rpm_threshold: Option<u32>,
}

/// `FanControlSettings` of a stored configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FanControlSettings {
    pub mode: FanControlMode,
    /// 0.0–1.0.
    pub static_speed: Option<f32>,
    pub temperature_key: Option<String>,
    pub interval_ms: Option<u64>,
    pub curve: FanCurveMap,
    pub spindown_delay_ms: Option<u64>,
    pub change_threshold: Option<u64>,
}

/// `get_gpu_config`: the configuration LACT stores for one GPU, **without** clocks, voltages,
/// power states and power profile modes.
///
/// Read-only: never send it back with `set_gpu_config`, which replaces the whole configuration
/// and would erase every field this subset does not carry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuConfig {
    pub fan_control_enabled: bool,
    pub fan_control_settings: Option<FanControlSettings>,
    pub pmfw_options: PmfwOptions,
    /// Watts; `None` = the GPU's default.
    pub power_cap: Option<f64>,
    pub performance_level: Option<String>,
}

/// `list_profiles` with `include_state: false`, without the profile rules and hooks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfilesInfo {
    /// The active profile; `None` = LACT's default profile.
    pub current_profile: Option<String>,
    pub auto_switch: bool,
}

// ---- Requests ----

/// The arguments of `set_fan_control` (`FanOptions` in lact-schema 0.10.1, without
/// `spindown_delay_ms` and `change_threshold`, which lactd then keeps as stored).
///
/// What lactd 0.10.1 does with it (`set_fan_control` in `lact-daemon/src/server/handler.rs`):
/// `enabled` replaces `fan_control_enabled` (false = the firmware's automatic control);
/// `mode: null` keeps the stored fan settings, `static` replaces the static speed (0.0–1.0, else
/// "static speed value out of range"), `curve` replaces the curve (speeds 0.0–1.0); **`pmfw`
/// replaces the stored PMFW options as a whole**, so a caller that changes one of them sends the
/// others as they are stored. lactd applies the four PMFW limits only while `enabled` is false,
/// and zero RPM only outside static mode (`apply_config` in `gpu_controller/amd.rs`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FanOptions {
    /// The LACT device id.
    pub id: String,
    pub enabled: bool,
    pub mode: Option<FanControlMode>,
    /// 0.0–1.0.
    pub static_speed: Option<f32>,
    pub curve: Option<FanCurveMap>,
    pub pmfw: PmfwOptions,
}

// ---- Client ----

pub struct LactClient {
    socket: PathBuf,
    timeout: Duration,
}

impl Default for LactClient {
    fn default() -> Self {
        Self::new(DEFAULT_SOCKET)
    }
}

impl LactClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The same client with another read and write timeout (tests use a short one).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sends a command and returns the `data` field of the response. One connection per
    /// request, as LACT's own CLI does.
    pub fn request(&self, command: &str, args: Option<Value>) -> LactResult<Value> {
        let mut req = json!({ "command": command });
        if let Some(args) = args {
            req["args"] = args;
        }

        let mut stream = UnixStream::connect(&self.socket).map_err(LactError::connecting)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(LactError::Io)?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(LactError::Io)?;

        let mut line = serde_json::to_vec(&req).map_err(|e| LactError::Protocol(e.to_string()))?;
        line.push(b'\n');
        stream.write_all(&line).map_err(LactError::Io)?;

        let mut buf = String::new();
        BufReader::new(stream)
            .read_line(&mut buf)
            .map_err(LactError::Io)?;
        parse_response(&buf)
    }

    fn call<T: DeserializeOwned>(&self, command: &str, args: Option<Value>) -> LactResult<T> {
        let data = self.request(command, args)?;
        serde_json::from_value(data).map_err(|e| LactError::Protocol(format!("{command}: {e}")))
    }

    pub fn ping(&self) -> LactResult<()> {
        self.request("ping", None).map(|_| ())
    }

    pub fn system_info(&self) -> LactResult<SystemInfo> {
        self.call("system_info", None)
    }

    pub fn list_devices(&self) -> LactResult<Vec<DeviceListEntry>> {
        self.call("list_devices", None)
    }

    /// Without the Vulkan/OpenCL details, which take LACT time to collect and tuning never reads.
    pub fn device_info(&self, id: &str) -> LactResult<DeviceInfo> {
        self.call(
            "device_info",
            Some(json!({ "id": id, "include_api_info": false })),
        )
    }

    pub fn device_stats(&self, id: &str) -> LactResult<DeviceStats> {
        self.call("device_stats", Some(json!({ "id": id })))
    }

    /// `None` when LACT has never stored a configuration for this GPU.
    pub fn get_gpu_config(&self, id: &str) -> LactResult<Option<GpuConfig>> {
        self.call("get_gpu_config", Some(json!({ "id": id })))
    }

    pub fn list_profiles(&self) -> LactResult<ProfilesInfo> {
        self.call("list_profiles", Some(json!({ "include_state": false })))
    }

    /// Sets the power cap in watts (`None` = default) and returns how many seconds LACT waits
    /// for [`confirm_pending`](Self::confirm_pending) before it reverts the change by itself.
    ///
    /// In 0.10.1 every setter goes through `edit_gpu_config`: it refuses with "There is an
    /// unconfirmed configuration change pending" while any change (on any GPU) is pending,
    /// applies the new configuration, and answers with `apply_settings_timer` (seconds, 5 by
    /// default, set in `/etc/lact/config.yaml`). Without `confirm` before that timer, lactd
    /// restores the previous configuration on its own.
    pub fn set_power_cap(&self, id: &str, watts: Option<f64>) -> LactResult<u64> {
        self.call("set_power_cap", Some(json!({ "id": id, "cap": watts })))
    }

    /// Sets the fan control and returns how many seconds LACT waits for
    /// [`confirm_pending`](Self::confirm_pending), like [`set_power_cap`](Self::set_power_cap).
    pub fn set_fan_control(&self, options: &FanOptions) -> LactResult<u64> {
        let args = serde_json::to_value(options).map_err(|e| LactError::Protocol(e.to_string()))?;
        self.call("set_fan_control", Some(args))
    }

    /// `confirm_pending_config` with `confirm` (keep and save the pending change) or `revert`
    /// (restore the previous configuration). lactd 0.10.1 holds a single pending change for the
    /// whole daemon, so the request names no GPU; it answers an error when nothing is pending
    /// any more ("No pending config changes", or "Could not confirm config" when its own timer
    /// settled the change a moment earlier).
    pub fn confirm_pending(&self, confirm: bool) -> LactResult<()> {
        let command = if confirm { "confirm" } else { "revert" };
        self.request(
            "confirm_pending_config",
            Some(json!({ "command": command })),
        )
        .map(|_| ())
    }
}

/// One response line: the `data` of `ok`, or the daemon's message for `error`.
pub fn parse_response(line: &str) -> LactResult<Value> {
    if line.trim().is_empty() {
        return Err(LactError::Protocol("empty answer".into()));
    }
    let resp: Value = serde_json::from_str(line)
        .map_err(|e| LactError::Protocol(format!("invalid JSON ({e})")))?;
    match resp["status"].as_str() {
        Some("ok") => Ok(resp["data"].clone()),
        Some("error") => Err(LactError::Daemon(error_message(&resp["data"]))),
        _ => Err(LactError::Protocol("no status in the answer".into())),
    }
}

/// LACT sends errors as `{ description, source: { description, source: … } }`
/// (`serde_error::Error`): the chain joined with `: `.
fn error_message(data: &Value) -> String {
    let mut parts = Vec::new();
    let mut node = data;
    while let Some(description) = node.get("description").and_then(Value::as_str) {
        parts.push(description.to_owned());
        node = &node["source"];
    }
    if parts.is_empty() {
        data.to_string()
    } else {
        parts.join(": ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    /// Shapes captured from lactd 0.10.1 on an RX 9070 XT (overdrive off).
    const SYSTEM_INFO: &str = r#"{"version":"0.10.1","commit":"30dc182","profile":"release","distro":"Ubuntu","kernel_version":"6.8.0","amdgpu_overdrive_enabled":false,"amdgpu_params_configurator":{"Modprobe":"Debian"}}"#;
    const DEVICES: &str = r#"[{"id":"1002:7550-1043:0643-0000:0d:00.0","name":"AMD Radeon RX 9070 XT","device_type":"Dedicated"}]"#;
    const STATS: &str = r#"{"fan":{"control_enabled":false,"pwm_current":0,"speed_current":0,"speed_max":6000,"speed_min":0,"pwm_max":255,"pwm_min":0,"pmfw_info":{}},"power":{"average":21.0,"cap_current":317.0,"cap_max":340.0,"cap_min":221.0,"cap_default":317.0},"temps":{"edge":{"current":40.0,"crit":110.0,"crit_hyst":-273.15,"primary":true,"display_only":false}},"busy_percent":3,"performance_level":"auto","active_power_states":{"core":"Sleep","memory":{"Index":0}},"throttle_info":{}}"#;
    /// The example configuration in lact-schema's own `config.rs` test.
    const CONFIG: &str = r#"{"fan_control_enabled":false,"fan_control_settings":{"mode":"curve","static_speed":0.5938412,"temperature_key":"edge","interval_ms":500,"curve":{"40":0.3,"50":0.35,"60":0.5,"70":0.75,"80":1.0},"spindown_delay_ms":1000,"change_threshold":2},"power_cap":318.0,"gpu_clock_offsets":{"0":-64}}"#;

    #[test]
    fn parses_the_system_info_of_lactd_0_10_1() {
        let info: SystemInfo = serde_json::from_str(SYSTEM_INFO).unwrap();
        assert_eq!(info.version, VERIFIED_VERSION);
        assert_eq!(info.amdgpu_overdrive_enabled, Some(false));
        assert_eq!(
            info.amdgpu_params_configurator,
            Some(json!({"Modprobe": "Debian"}))
        );
    }

    #[test]
    fn the_pci_slot_is_the_end_of_the_lact_id() {
        let devices: Vec<DeviceListEntry> = serde_json::from_str(DEVICES).unwrap();
        assert_eq!(devices[0].pci_slot(), "0000:0d:00.0");
        assert_eq!(devices[0].device_type, DeviceType::Dedicated);
        assert!(find_device(&devices, "0000:0d:00.0").is_some());
        assert!(find_device(&devices, "0000:0d:00.1").is_none());
        // A suffix of the slot is not the slot.
        assert!(find_device(&devices, "0d:00.0").is_none());
    }

    #[test]
    fn device_stats_without_pmfw_leave_every_pmfw_value_absent() {
        let stats: DeviceStats = serde_json::from_str(STATS).unwrap();
        assert_eq!(stats.power.cap_current, Some(317.0));
        assert_eq!(stats.power.cap_min, Some(221.0));
        assert_eq!(stats.power.current, None);
        assert_eq!(stats.fan.pmfw_info, PmfwInfo::default());
        assert_eq!(stats.fan.control_mode, None);
        assert_eq!(stats.fan.curve, None);
        assert_eq!(stats.performance_level.as_deref(), Some("auto"));
    }

    #[test]
    fn device_stats_with_pmfw_and_a_curve() {
        let stats: DeviceStats = serde_json::from_value(json!({
            "fan": {
                "control_enabled": true, "control_mode": "curve", "static_speed": 0.5,
                "curve": {"40": 0.3, "80": 1.0}, "temperature_range": [25, 100],
                "pmfw_info": {
                    "acoustic_limit": {"current": 2450, "allowed_range": [500, 3200]},
                    "minimum_pwm": {"current": 15, "allowed_range": null},
                    "zero_rpm_enable": true
                }
            },
            "power": {}
        }))
        .unwrap();
        let fan = stats.fan;
        assert_eq!(fan.control_mode, Some(FanControlMode::Curve));
        assert_eq!(fan.curve.unwrap().get(&80), Some(&1.0));
        assert_eq!(fan.temperature_range, Some((25, 100)));
        assert_eq!(
            fan.pmfw_info.acoustic_limit,
            Some(FanInfo {
                current: 2450,
                allowed_range: Some((500, 3200))
            })
        );
        assert_eq!(fan.pmfw_info.minimum_pwm.unwrap().allowed_range, None);
        assert_eq!(fan.pmfw_info.zero_rpm_enable, Some(true));
        assert_eq!(fan.pmfw_info.target_temp, None);
    }

    #[test]
    fn parses_a_stored_gpu_config_and_ignores_what_it_does_not_carry() {
        let config: GpuConfig = serde_json::from_str(CONFIG).unwrap();
        assert_eq!(config.power_cap, Some(318.0));
        let fan = config.fan_control_settings.unwrap();
        assert_eq!(fan.mode, FanControlMode::Curve);
        assert_eq!(fan.curve.len(), 5);
        assert_eq!(fan.change_threshold, Some(2));
        let never: Option<GpuConfig> = serde_json::from_value(Value::Null).unwrap();
        assert_eq!(never, None);
    }

    #[test]
    fn unknown_device_flags_are_kept_by_name() {
        let info: DeviceInfo = serde_json::from_value(json!({
            "driver": "amdgpu", "flags": ["DumpableVBios", "HasPmfw", "SomethingNew"],
            "pci_info": {}, "link_info": {}
        }))
        .unwrap();
        assert!(info.has(&DeviceFlag::HasPmfw));
        assert!(!info.has(&DeviceFlag::ConfigurableFanControl));
        assert_eq!(info.flags[2], DeviceFlag::Other("SomethingNew".into()));
        assert_eq!(serde_json::to_value(&info.flags[1]).unwrap(), "HasPmfw");
    }

    #[test]
    fn unknown_fan_modes_and_device_types_do_not_fail_the_answer() {
        let stats: DeviceStats =
            serde_json::from_value(json!({ "fan": { "control_mode": "auto" }, "power": {} }))
                .unwrap();
        assert_eq!(
            stats.fan.control_mode,
            Some(FanControlMode::Other("auto".into()))
        );
        let devices: Vec<DeviceListEntry> =
            serde_json::from_value(json!([{ "id": "x-0000:0d:00.0", "device_type": "Virtual" }]))
                .unwrap();
        assert_eq!(devices[0].device_type, DeviceType::Other("Virtual".into()));
        assert_eq!(
            serde_json::to_value(FanControlMode::Static).unwrap(),
            "static"
        );
        assert_eq!(
            serde_json::to_value(DeviceType::Integrated).unwrap(),
            "Integrated"
        );
    }

    #[test]
    fn error_answers_carry_the_daemon_message_chain() {
        let err = parse_response(
            r#"{"status":"error","data":{"description":"Failed to deserialize","source":{"description":"missing field `args`","source":null}}}"#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, LactError::Daemon(m) if m == "Failed to deserialize: missing field `args`"),
            "{err:?}"
        );
        assert!(matches!(
            parse_response("not json"),
            Err(LactError::Protocol(_))
        ));
        assert!(matches!(parse_response(""), Err(LactError::Protocol(_))));
        assert!(matches!(
            parse_response(r#"{"data":1}"#),
            Err(LactError::Protocol(_))
        ));
    }

    /// A temporary folder removed at the end of the test, also on panic.
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn temp(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("redline-lact-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    #[test]
    fn a_missing_socket_and_a_stale_one_are_told_apart() {
        let dir = temp("connect");
        let socket = dir.0.join("lactd.sock");
        let client = LactClient::new(&socket).with_timeout(Duration::from_secs(2));
        assert!(matches!(client.ping(), Err(LactError::SocketMissing)));

        // A socket file with no listener behind it: lactd stopped without removing it.
        drop(UnixListener::bind(&socket).unwrap());
        assert!(matches!(client.ping(), Err(LactError::NotRunning)));
    }

    #[test]
    fn sends_one_request_line_and_reads_the_typed_answer() {
        let dir = temp("roundtrip");
        let socket = dir.0.join("lactd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut stream = stream;
            writeln!(stream, r#"{{"status":"ok","data":{DEVICES}}}"#).unwrap();
            line
        });
        let client = LactClient::new(&socket).with_timeout(Duration::from_secs(2));
        let devices = client.list_devices().unwrap();
        assert_eq!(devices[0].name.as_deref(), Some("AMD Radeon RX 9070 XT"));
        let request: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(request, json!({ "command": "list_devices" }));
    }

    /// Answers one request with `answer` and returns the request line it read.
    fn answer_once(listener: UnixListener, answer: &'static str) -> thread::JoinHandle<Value> {
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let mut stream = stream;
            writeln!(stream, "{answer}").unwrap();
            serde_json::from_str(&line).unwrap()
        })
    }

    #[test]
    fn the_setter_and_the_confirmation_use_the_0_10_1_request_shapes() {
        let dir = temp("setters");
        let socket = dir.0.join("lactd.sock");
        let client = LactClient::new(&socket).with_timeout(Duration::from_secs(2));

        // `set_power_cap` answers `apply_settings_timer`, in seconds.
        let server = answer_once(
            UnixListener::bind(&socket).unwrap(),
            r#"{"status":"ok","data":5}"#,
        );
        assert_eq!(
            client.set_power_cap("x-0000:0d:00.0", Some(250.0)).unwrap(),
            5
        );
        assert_eq!(
            server.join().unwrap(),
            json!({ "command": "set_power_cap", "args": { "id": "x-0000:0d:00.0", "cap": 250.0 } })
        );
        std::fs::remove_file(&socket).unwrap();

        let server = answer_once(
            UnixListener::bind(&socket).unwrap(),
            r#"{"status":"ok","data":null}"#,
        );
        client.confirm_pending(false).unwrap();
        assert_eq!(
            server.join().unwrap(),
            json!({ "command": "confirm_pending_config", "args": { "command": "revert" } })
        );
    }

    #[test]
    fn set_fan_control_sends_lacts_fan_options() {
        let dir = temp("fan");
        let socket = dir.0.join("lactd.sock");
        let client = LactClient::new(&socket).with_timeout(Duration::from_secs(2));
        let server = answer_once(
            UnixListener::bind(&socket).unwrap(),
            r#"{"status":"ok","data":5}"#,
        );
        let options = FanOptions {
            id: "x-0000:0d:00.0".into(),
            enabled: true,
            mode: Some(FanControlMode::Curve),
            static_speed: None,
            curve: Some([(40, 0.25), (80, 1.0)].into()),
            pmfw: PmfwOptions {
                zero_rpm: Some(false),
                ..PmfwOptions::default()
            },
        };
        assert_eq!(client.set_fan_control(&options).unwrap(), 5);
        // The shape lact-schema 0.10.1 deserializes `Request::SetFanControl(FanOptions)` from:
        // missing options are `null`, curve keys are strings.
        assert_eq!(
            server.join().unwrap(),
            json!({
                "command": "set_fan_control",
                "args": {
                    "id": "x-0000:0d:00.0", "enabled": true, "mode": "curve",
                    "static_speed": null, "curve": { "40": 0.25, "80": 1.0 },
                    "pmfw": {
                        "acoustic_limit": null, "acoustic_target": null, "minimum_pwm": null,
                        "target_temperature": null, "zero_rpm": false, "zero_rpm_threshold": null
                    }
                }
            })
        );
    }

    #[test]
    fn a_silent_daemon_times_out_instead_of_hanging() {
        let dir = temp("silent");
        let socket = dir.0.join("lactd.sock");
        // Bound and listening (the backlog accepts the connection), never answering.
        let _listener = UnixListener::bind(&socket).unwrap();
        let client = LactClient::new(&socket).with_timeout(Duration::from_millis(200));
        assert!(matches!(client.ping(), Err(LactError::Io(_))));
    }
}
