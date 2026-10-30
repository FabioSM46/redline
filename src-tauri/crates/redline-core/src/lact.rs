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
//! | `device_clocks_info` | `{ id }` | [`ClocksInfo`] (a refusal = no overdrive table) |
//! | `get_gpu_config` | `{ id }` | `Option<`[`GpuConfig`]`>` (`null` = never configured) |
//! | `list_profiles` | `{ include_state: false }` | [`ProfilesInfo`] |
//! | `set_power_cap` | `{ id, cap }` (`cap: null` = default) | seconds until lactd reverts |
//! | `set_fan_control` | [`FanOptions`] | seconds until lactd reverts |
//! | `batch_set_clocks_value` | `{ id, commands }` ([`ClockCommand`]s) | seconds until lactd reverts |
//! | `set_clocks_value` | `{ id, command: { type: "reset", value: null } }` | seconds until lactd reverts |
//! | `confirm_pending_config` | `{ command: "confirm" \| "revert" }` | — |
//! | `set_profile` | `{ name, auto_switch: false }` (`name: null` = default profile) | — |
//! | `create_profile` | `{ name, base }` ([`ProfileBase`]) | — |
//! | `delete_profile` | `{ name }` | — |
//! | `enable_overdrive` | — | — (an empty string) |
//! | `disable_overdrive` | — | — (an empty string) |

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
/// How long `enable_overdrive` and `disable_overdrive` may wait for the answer: lactd answers
/// only after regenerating the initramfs, which takes minutes with `dracut --regenerate-all`.
pub const OVERDRIVE_TIMEOUT: Duration = Duration::from_secs(600);

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
    /// `{"Modprobe": "Debian"}`); `None` when LACT cannot set kernel parameters here (NixOS, or
    /// an unreadable `/etc/os-release`). Kept untyped so that a configurator a later LACT adds
    /// does not fail the whole answer: read it with [`params_configurator`](Self::params_configurator).
    pub amdgpu_params_configurator: Option<Value>,
}

impl SystemInfo {
    /// [`amdgpu_params_configurator`](Self::amdgpu_params_configurator), typed. `None` when LACT
    /// cannot set kernel parameters on this system.
    pub fn params_configurator(&self) -> Option<ParamsConfigurator> {
        let value = self.amdgpu_params_configurator.as_ref()?;
        if value.is_null() {
            return None;
        }
        let unknown = || ParamsConfigurator::Other(value.to_string());
        if let Some(initramfs) = value.get("Modprobe") {
            return Some(match initramfs {
                Value::Null => ParamsConfigurator::Modprobe(None),
                Value::String(name) => {
                    ParamsConfigurator::Modprobe(Some(InitramfsType::from(name.clone())))
                }
                _ => unknown(),
            });
        }
        Some(match value.get("BootArg").and_then(Value::as_str) {
            Some("RpmOstree") => ParamsConfigurator::RpmOstree,
            _ => unknown(),
        })
    }
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

/// A range of the overdrive table (`amdgpu_sysfs::…::Range`); either end may be missing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ClockRange {
    pub min: Option<i32>,
    pub max: Option<i32>,
}

impl ClockRange {
    /// `(min, max)` when the range has both ends in order. lactd's own checks need both too
    /// (`check_clockspeed_in_range` in `amdgpu-sysfs` 0.21.0 refuses a partial range).
    pub fn full(self) -> Option<(i32, i32)> {
        self.min.zip(self.max).filter(|(min, max)| min <= max)
    }
}

/// The `od_range` of an RDNA table: what the firmware allows. In MHz, the offsets in mV for
/// voltage. Each is `None` when the firmware does not expose it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RdnaOdRange {
    /// RDNA1–3.
    pub sclk: Option<ClockRange>,
    /// RDNA4.
    pub sclk_offset: Option<ClockRange>,
    /// Discrete GPUs only.
    pub mclk: Option<ClockRange>,
    /// When present, its last entry is what lactd checks a maximum core clock against
    /// (`get_max_sclk_range`), before `sclk`.
    pub curve_sclk_points: Vec<ClockRange>,
    /// RDNA3 and later: RDNA2 has a voltage offset but gives no range for it.
    pub voltage_offset: Option<ClockRange>,
}

/// The overdrive table of the format `amdgpu-sysfs` calls RDNA (Vega20 and newer), without the
/// V/F curve.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RdnaClocksTable {
    /// MHz. Empty on RDNA4, which uses [`sclk_offset`](Self::sclk_offset).
    pub current_sclk_range: ClockRange,
    /// MHz.
    pub sclk_offset: Option<i32>,
    /// MHz. Empty on integrated GPUs.
    pub current_mclk_range: ClockRange,
    /// mV.
    pub voltage_offset: Option<i32>,
    pub od_range: RdnaOdRange,
}

/// LACT's `ClocksTable`, with `ClocksTableGen` flattened into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClocksTable {
    Rdna(RdnaClocksTable),
    /// The older format (Vega10 and earlier). Nothing in it is read.
    Gcn,
    /// A table of another vendor, of a shape this client does not know, or unreadable: kept out
    /// of the way instead of failing the whole answer.
    Other,
}

impl ClocksTable {
    /// On the wire `{ "type": "amd", "value": { "kind": "rdna", "data": {...} } }`: two nested
    /// tags, `ClocksTable` in lact-schema and `ClocksTableGen` in `amdgpu-sysfs` 0.21.0.
    fn from_wire(table: Value) -> Self {
        let value = &table["value"];
        match (table["type"].as_str(), value["kind"].as_str()) {
            (Some("amd"), Some("gcn")) => ClocksTable::Gcn,
            (Some("amd"), Some("rdna")) => serde_json::from_value(value["data"].clone())
                .map_or(ClocksTable::Other, ClocksTable::Rdna),
            _ => ClocksTable::Other,
        }
    }
}

/// `device_clocks_info`, without `max_sclk`, `max_mclk` and `max_voltage`, which repeat the table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ClocksInfo {
    #[serde(deserialize_with = "clocks_table")]
    pub table: Option<ClocksTable>,
}

fn clocks_table<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ClocksTable>, D::Error> {
    let table = Option::<Value>::deserialize(deserializer)?;
    Ok(table.map(ClocksTable::from_wire))
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

/// The clock and voltage values of a stored configuration: `ClocksConfiguration` in lact-schema
/// 0.10.1, which `get_gpu_config` flattens into [`GpuConfig`]. The offsets and curves are maps
/// keyed by a number; their values matter here only by being there.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredClocks {
    pub min_core_clock: Option<i32>,
    pub min_memory_clock: Option<i32>,
    pub min_voltage: Option<i32>,
    pub max_core_clock: Option<i32>,
    pub max_memory_clock: Option<i32>,
    pub max_voltage: Option<i32>,
    pub voltage_offset: Option<i32>,
    pub voltage_boost: Option<i32>,
    pub gpu_clock_offsets: BTreeMap<String, Value>,
    pub mem_clock_offsets: BTreeMap<String, Value>,
    pub gpu_vf_curve: BTreeMap<String, Value>,
    pub mem_vf_curve: BTreeMap<String, Value>,
    pub nvidia_gpu_vf_curve: BTreeMap<String, Value>,
}

impl StoredClocks {
    /// LACT stores no value: `is_core_clocks_used` in lact-schema 0.10.1 is its opposite.
    pub fn is_empty(&self) -> bool {
        *self == StoredClocks::default()
    }
}

/// `get_gpu_config`: the configuration LACT stores for one GPU, **without** power states and
/// power profile modes. lactd 0.10.1 reads it from the active profile (`Config::gpus`), or from
/// its default configuration when no profile is active.
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
    #[serde(flatten)]
    pub clocks: StoredClocks,
}

/// `list_profiles` with `include_state: false`, without the profile rules and hooks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfilesInfo {
    /// The names of the stored profiles, sorted. LACT sends them as the keys of a map
    /// (`profiles: { name: rule }`); the rules are not read.
    #[serde(deserialize_with = "profile_names")]
    pub profiles: Vec<String>,
    /// The active profile; `None` = LACT's default profile.
    pub current_profile: Option<String>,
    pub auto_switch: bool,
}

/// The keys of LACT's `profiles` map, sorted by name.
fn profile_names<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    let map = BTreeMap::<String, serde::de::IgnoredAny>::deserialize(deserializer)?;
    Ok(map.into_keys().collect())
}

/// What a new profile starts from (`ProfileBase` in lact-schema 0.10.1, without `Empty` and
/// `Provided`, which Redline never sends).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileBase {
    /// A copy of LACT's default profile (the configuration used when no profile is active).
    Default,
    /// A copy of the named profile.
    Profile(String),
}

// ---- Requests ----

/// What a [`ClockCommand`] sets: the subset of LACT's `ClockspeedType` that Redline offers. Its
/// `reset` is left out on purpose: at 0.10.1 only `set_clocks_value` acts on it, so a batch cannot
/// carry one ([`LactClient::reset_clocks`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSetting {
    MaxCoreClock,
    /// The core clock offset of one power state. On RDNA lactd reads state 0 only.
    GpuClockOffset(u32),
    MaxMemoryClock,
    VoltageOffset,
}

/// One entry of `batch_set_clocks_value`: `SetClocksCommand` in lact-schema. `value: None` clears
/// the setting from lactd's configuration, which leaves it at the firmware's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClockCommand {
    #[serde(rename = "type")]
    pub setting: ClockSetting,
    pub value: Option<i32>,
}

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

/// How lactd 0.10.1 sets `amdgpu.ppfeaturemask` (`AmdgpuParamsConfigurator` in lact-schema,
/// `enable_overdrive` / `disable_overdrive` in `lact-daemon/src/system.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamsConfigurator {
    /// Writes (or, to disable, removes) `/etc/modprobe.d/99-amdgpu-overdrive.conf`, then
    /// regenerates the initramfs with the given tool; `None` = no initramfs to regenerate.
    Modprobe(Option<InitramfsType>),
    /// Adds (or removes) `amdgpu.ppfeaturemask` in the kernel arguments with `rpm-ostree kargs`
    /// (Fedora Atomic).
    RpmOstree,
    /// A configurator this client does not know, as LACT sent it.
    Other(String),
}

/// The file lactd 0.10.1 writes to enable overdrive with [`ParamsConfigurator::Modprobe`].
pub const OVERDRIVE_MODPROBE_FILE: &str = "/etc/modprobe.d/99-amdgpu-overdrive.conf";

open_enum! {
    /// LACT's `InitramfsType`: which tool regenerates the initramfs.
    InitramfsType {
        /// `update-initramfs -u` (Debian, Ubuntu).
        Debian = "Debian",
        /// `mkinitcpio -P` (Arch).
        Mkinitcpio = "Mkinitcpio",
        /// `dracut --regenerate-all --force` (Fedora).
        Dracut = "Dracut",
    }
}

impl InitramfsType {
    /// The command lactd runs, or `None` for a type this client does not know.
    pub fn command(&self) -> Option<&'static str> {
        match self {
            InitramfsType::Debian => Some("update-initramfs -u"),
            InitramfsType::Mkinitcpio => Some("mkinitcpio -P"),
            InitramfsType::Dracut => Some("dracut --regenerate-all --force"),
            InitramfsType::Other(_) => None,
        }
    }
}

// ---- Client ----

pub struct LactClient {
    socket: PathBuf,
    timeout: Duration,
    overdrive_timeout: Duration,
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
            overdrive_timeout: OVERDRIVE_TIMEOUT,
        }
    }

    /// The same client with another read and write timeout (tests use a short one).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The same client with another timeout for the overdrive requests (tests use a short one).
    pub fn with_overdrive_timeout(mut self, timeout: Duration) -> Self {
        self.overdrive_timeout = timeout;
        self
    }

    /// Sends a command and returns the `data` field of the response. One connection per
    /// request, as LACT's own CLI does.
    pub fn request(&self, command: &str, args: Option<Value>) -> LactResult<Value> {
        self.request_within(command, args, self.timeout)
    }

    fn request_within(
        &self,
        command: &str,
        args: Option<Value>,
        timeout: Duration,
    ) -> LactResult<Value> {
        let mut req = json!({ "command": command });
        if let Some(args) = args {
            req["args"] = args;
        }

        let mut stream = UnixStream::connect(&self.socket).map_err(LactError::connecting)?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(LactError::Io)?;
        stream
            .set_write_timeout(Some(timeout))
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

    /// The overdrive table of a GPU. Read-only. Without one lactd 0.10.1 refuses with "Clocks
    /// table not available" (`get_clocks_info` in `gpu_controller/amd.rs`): that is the expected
    /// answer with overdrive off, not a failure.
    pub fn device_clocks_info(&self, id: &str) -> LactResult<ClocksInfo> {
        self.call("device_clocks_info", Some(json!({ "id": id })))
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

    /// Activates the profile `name` (`None` = the default profile), with LACT's automatic
    /// switching off.
    ///
    /// In 0.10.1 this is **not** a pending change: `set_profile` stops the profile watcher,
    /// applies the profile's configuration to every GPU at once and saves it, with no timer and
    /// no `confirm_pending_config`. It does not check lactd's pending change either. The caller
    /// owns the confirmation. It refuses with "Cannot change profile while a hold is active".
    pub fn set_profile(&self, name: Option<&str>) -> LactResult<()> {
        self.request(
            "set_profile",
            Some(json!({ "name": name, "auto_switch": false })),
        )
        .map(|_| ())
    }

    /// Stores a new profile copied from `base` and saves LACT's configuration; nothing is
    /// applied to the GPU. Refuses an existing name ("Profile <name> already exists").
    pub fn create_profile(&self, name: &str, base: &ProfileBase) -> LactResult<()> {
        self.request(
            "create_profile",
            Some(json!({ "name": name, "base": base })),
        )
        .map(|_| ())
    }

    /// Removes a profile and saves LACT's configuration. In 0.10.1, deleting the **active**
    /// profile first switches to the default one (applied at once, unconfirmed); a name that
    /// does not exist is not an error.
    pub fn delete_profile(&self, name: &str) -> LactResult<()> {
        self.request("delete_profile", Some(json!({ "name": name })))
            .map(|_| ())
    }

    /// Sets the fan control and returns how many seconds LACT waits for
    /// [`confirm_pending`](Self::confirm_pending), like [`set_power_cap`](Self::set_power_cap).
    pub fn set_fan_control(&self, options: &FanOptions) -> LactResult<u64> {
        let args = serde_json::to_value(options).map_err(|e| LactError::Protocol(e.to_string()))?;
        self.call("set_fan_control", Some(args))
    }

    /// Applies `commands` as one change and returns how many seconds LACT waits for
    /// [`confirm_pending`](Self::confirm_pending), like [`set_power_cap`](Self::set_power_cap).
    pub fn batch_set_clocks(&self, id: &str, commands: &[ClockCommand]) -> LactResult<u64> {
        self.call(
            "batch_set_clocks_value",
            Some(json!({ "id": id, "commands": commands })),
        )
    }

    /// Returns the GPU's clocks to the firmware's values and clears them from lactd's
    /// configuration, pending like any other setter.
    ///
    /// In 0.10.1 only `set_clocks_value` calls `reset_clocks()`: `batch_set_clocks_value` would
    /// clear the stored values without resetting the table, so a reset is always sent alone. That
    /// call also runs **before** `edit_gpu_config` checks for a pending change: while another
    /// program holds one, lactd resets the table and then refuses.
    pub fn reset_clocks(&self, id: &str) -> LactResult<u64> {
        self.call(
            "set_clocks_value",
            Some(json!({ "id": id, "command": { "type": "reset", "value": null } })),
        )
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

    /// `enable_overdrive`: lactd (root) sets the overdrive bit of `amdgpu.ppfeaturemask` in the
    /// boot configuration, as [`SystemInfo::params_configurator`] says, and regenerates the
    /// initramfs. It takes effect after a reboot. Not a pending change: there is nothing to
    /// confirm with lactd, so the caller asks the user **before** sending it.
    ///
    /// Waits up to the overdrive timeout ([`OVERDRIVE_TIMEOUT`] by default). lactd 0.10.1
    /// refuses with "Overdrive support was already toggled - please reboot to apply the
    /// changes" after any enable or disable since it started, and with "Overdrive mask already
    /// enabled" when the running kernel already has the bit.
    pub fn enable_overdrive(&self) -> LactResult<()> {
        self.request_within("enable_overdrive", None, self.overdrive_timeout)
            .map(|_| ())
    }

    /// `disable_overdrive`: undoes [`enable_overdrive`](Self::enable_overdrive) (removes the
    /// modprobe file and regenerates the initramfs, or removes the kernel argument). lactd
    /// 0.10.1 refuses with "Overclocking was not enabled through LACT (…)" when the modprobe
    /// file is missing, and with the same "already toggled" message as enabling.
    pub fn disable_overdrive(&self) -> LactResult<()> {
        self.request_within("disable_overdrive", None, self.overdrive_timeout)
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
        assert!(!config.clocks.is_empty());
        assert!(config.clocks.gpu_clock_offsets.contains_key("0"));
        assert!(GpuConfig::default().clocks.is_empty());
        let voltage: GpuConfig = serde_json::from_str(r#"{"voltage_offset":-50}"#).unwrap();
        assert_eq!(voltage.clocks.voltage_offset, Some(-50));
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
    fn the_clock_requests_use_the_0_10_1_shapes() {
        let dir = temp("clock-requests");
        let socket = dir.0.join("lactd.sock");
        let client = LactClient::new(&socket).with_timeout(Duration::from_secs(2));

        let server = answer_once(
            UnixListener::bind(&socket).unwrap(),
            r#"{"status":"ok","data":5}"#,
        );
        let command = |setting, value| ClockCommand { setting, value };
        let commands = [
            command(ClockSetting::MaxCoreClock, Some(2600)),
            command(ClockSetting::GpuClockOffset(0), Some(-100)),
            command(ClockSetting::MaxMemoryClock, Some(1300)),
            command(ClockSetting::VoltageOffset, None),
        ];
        assert_eq!(client.batch_set_clocks("gpu", &commands).unwrap(), 5);
        assert_eq!(
            server.join().unwrap(),
            json!({
                "command": "batch_set_clocks_value",
                "args": {
                    "id": "gpu",
                    "commands": [
                        { "type": "max_core_clock", "value": 2600 },
                        { "type": { "gpu_clock_offset": 0 }, "value": -100 },
                        { "type": "max_memory_clock", "value": 1300 },
                        { "type": "voltage_offset", "value": null }
                    ]
                }
            })
        );
        std::fs::remove_file(&socket).unwrap();

        // `reset` goes through the single command: the batch never calls `reset_clocks()`.
        let server = answer_once(
            UnixListener::bind(&socket).unwrap(),
            r#"{"status":"ok","data":5}"#,
        );
        assert_eq!(client.reset_clocks("gpu").unwrap(), 5);
        assert_eq!(
            server.join().unwrap(),
            json!({
                "command": "set_clocks_value",
                "args": { "id": "gpu", "command": { "type": "reset", "value": null } }
            })
        );
    }

    #[test]
    fn the_profile_list_is_the_sorted_names_without_the_rules() {
        let info: ProfilesInfo = serde_json::from_str(
            r#"{"profiles":{"Quiet":null,"Gaming":{"type":"process","filter":{"name":"game"}}},"profile_hooks":{},"current_profile":"Quiet","auto_switch":false,"watcher_state":null}"#,
        )
        .unwrap();
        assert_eq!(info.profiles, vec!["Gaming", "Quiet"]);
        assert_eq!(info.current_profile.as_deref(), Some("Quiet"));
        assert!(!info.auto_switch);
        let none: ProfilesInfo =
            serde_json::from_value(json!({ "current_profile": null })).unwrap();
        assert_eq!(none, ProfilesInfo::default());
    }

    #[test]
    fn the_profile_requests_use_the_0_10_1_shapes() {
        let dir = temp("profiles");
        let socket = dir.0.join("lactd.sock");
        let client = LactClient::new(&socket).with_timeout(Duration::from_secs(2));
        let null = r#"{"status":"ok","data":null}"#;

        let server = answer_once(UnixListener::bind(&socket).unwrap(), null);
        client.set_profile(Some("Quiet")).unwrap();
        assert_eq!(
            server.join().unwrap(),
            json!({ "command": "set_profile", "args": { "name": "Quiet", "auto_switch": false } })
        );
        std::fs::remove_file(&socket).unwrap();

        let server = answer_once(UnixListener::bind(&socket).unwrap(), null);
        client.set_profile(None).unwrap();
        assert_eq!(
            server.join().unwrap(),
            json!({ "command": "set_profile", "args": { "name": null, "auto_switch": false } })
        );
        std::fs::remove_file(&socket).unwrap();

        let server = answer_once(UnixListener::bind(&socket).unwrap(), null);
        client
            .create_profile("Loud", &ProfileBase::Profile("Quiet".into()))
            .unwrap();
        assert_eq!(
            server.join().unwrap(),
            json!({ "command": "create_profile", "args": { "name": "Loud", "base": { "profile": "Quiet" } } })
        );
        std::fs::remove_file(&socket).unwrap();

        let server = answer_once(UnixListener::bind(&socket).unwrap(), null);
        client
            .create_profile("Loud", &ProfileBase::Default)
            .unwrap();
        assert_eq!(server.join().unwrap()["args"]["base"], json!("default"));
        std::fs::remove_file(&socket).unwrap();

        let server = answer_once(
            UnixListener::bind(&socket).unwrap(),
            r#"{"status":"error","data":{"description":"Cannot delete profile 'Loud' while it is held","source":null}}"#,
        );
        let error = client.delete_profile("Loud").unwrap_err();
        assert!(matches!(&error, LactError::Daemon(m) if m.contains("while it is held")));
        assert_eq!(
            server.join().unwrap(),
            json!({ "command": "delete_profile", "args": { "name": "Loud" } })
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
    fn overdrive_requests_have_no_arguments_and_their_own_timeout() {
        let dir = temp("overdrive");
        let socket = dir.0.join("lactd.sock");
        // The ordinary timeout is far too short for the answer below: the overdrive one is used.
        let client = LactClient::new(&socket)
            .with_timeout(Duration::from_millis(50))
            .with_overdrive_timeout(Duration::from_secs(2));
        for (command, call) in [
            (
                "enable_overdrive",
                LactClient::enable_overdrive as fn(&LactClient) -> _,
            ),
            ("disable_overdrive", LactClient::disable_overdrive),
        ] {
            let listener = UnixListener::bind(&socket).unwrap();
            let server = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                // lactd answers only once the initramfs is regenerated.
                thread::sleep(Duration::from_millis(200));
                let mut stream = stream;
                writeln!(stream, r#"{{"status":"ok","data":""}}"#).unwrap();
                serde_json::from_str::<Value>(&line).unwrap()
            });
            call(&client).unwrap();
            assert_eq!(server.join().unwrap(), json!({ "command": command }));
            std::fs::remove_file(&socket).unwrap();
        }

        // A refusal is lactd's own message.
        let server = answer_once(
            UnixListener::bind(&socket).unwrap(),
            r#"{"status":"error","data":{"description":"Overdrive mask already enabled","source":null}}"#,
        );
        assert!(matches!(
            client.enable_overdrive(),
            Err(LactError::Daemon(m)) if m == "Overdrive mask already enabled"
        ));
        server.join().unwrap();
    }

    #[test]
    fn reads_how_lact_sets_the_kernel_parameter() {
        let configurator = |value: Value| {
            serde_json::from_value::<SystemInfo>(json!({
                "version": "0.10.1", "amdgpu_params_configurator": value
            }))
            .unwrap()
            .params_configurator()
        };
        assert_eq!(
            configurator(json!({ "Modprobe": "Debian" })),
            Some(ParamsConfigurator::Modprobe(Some(InitramfsType::Debian)))
        );
        assert_eq!(
            configurator(json!({ "Modprobe": "Dracut" })),
            Some(ParamsConfigurator::Modprobe(Some(InitramfsType::Dracut)))
        );
        assert_eq!(
            configurator(json!({ "Modprobe": null })),
            Some(ParamsConfigurator::Modprobe(None))
        );
        assert_eq!(
            configurator(json!({ "BootArg": "RpmOstree" })),
            Some(ParamsConfigurator::RpmOstree)
        );
        // NixOS, or an unreadable os-release: LACT cannot set it.
        assert_eq!(configurator(Value::Null), None);
        let info: SystemInfo = serde_json::from_str(r#"{"version":"0.10.1"}"#).unwrap();
        assert_eq!(info.params_configurator(), None);
        // What a later LACT may add is kept, not a failure.
        assert_eq!(
            configurator(json!({ "Modprobe": "Booster" })),
            Some(ParamsConfigurator::Modprobe(Some(InitramfsType::Other(
                "Booster".into()
            ))))
        );
        assert_eq!(InitramfsType::Other("Booster".into()).command(), None);
        assert_eq!(
            configurator(json!({ "BootArg": "Grubby" })),
            Some(ParamsConfigurator::Other(r#"{"BootArg":"Grubby"}"#.into()))
        );
        assert!(matches!(
            configurator(json!("Systemd")),
            Some(ParamsConfigurator::Other(_))
        ));
    }

    /// `parse_7900xt_full` of `amdgpu-sysfs` 0.21.0 under lact-schema 0.10.1's two tags.
    const CLOCKS_RDNA3: &str = r#"{"max_sclk":2735,"max_mclk":1250,"max_voltage":null,"table":{"type":"amd","value":{"kind":"rdna","data":{"current_sclk_range":{"min":500,"max":2735},"sclk_offset":null,"current_mclk_range":{"min":97,"max":1250},"vddc_curve":[],"voltage_offset":0,"od_range":{"sclk":{"min":500,"max":5000},"sclk_offset":null,"mclk":{"min":97,"max":1500},"curve_sclk_points":[],"curve_voltage_points":[],"voltage_offset":{"min":-450,"max":0}}}}}}"#;

    #[test]
    fn parses_both_tags_of_an_rdna_clocks_table() {
        let info: ClocksInfo = serde_json::from_str(CLOCKS_RDNA3).unwrap();
        let Some(ClocksTable::Rdna(table)) = info.table else {
            panic!("{info:?}");
        };
        assert_eq!(table.current_sclk_range.full(), Some((500, 2735)));
        assert_eq!(table.sclk_offset, None);
        assert_eq!(table.voltage_offset, Some(0));
        assert_eq!(table.od_range.sclk.unwrap().full(), Some((500, 5000)));
        assert_eq!(table.od_range.sclk_offset, None);
        assert_eq!(
            table.od_range.voltage_offset.unwrap().full(),
            Some((-450, 0))
        );
    }

    #[test]
    fn other_clocks_tables_do_not_fail_the_answer() {
        let table = |value: Value| {
            serde_json::from_value::<ClocksInfo>(json!({ "table": value }))
                .unwrap()
                .table
        };
        assert_eq!(
            table(json!({ "type": "amd", "value": { "kind": "gcn", "data": {
                "sclk_levels": [], "mclk_levels": [],
                "od_range": { "sclk": { "min": 852, "max": 2400 } }
            } } })),
            Some(ClocksTable::Gcn)
        );
        assert_eq!(
            table(json!({ "type": "nvidia", "value": { "gpu_offsets": {} } })),
            Some(ClocksTable::Other)
        );
        assert_eq!(
            table(json!({ "type": "amd", "value": { "kind": "rdna5", "data": {} } })),
            Some(ClocksTable::Other)
        );
        // A field of an unexpected type loses the table, not the answer.
        assert_eq!(
            table(json!({ "type": "amd", "value": { "kind": "rdna", "data": {
                "od_range": "none"
            } } })),
            Some(ClocksTable::Other)
        );
        assert_eq!(table(Value::Null), None);
        assert_eq!(
            serde_json::from_str::<ClocksInfo>("{}").unwrap(),
            ClocksInfo::default()
        );
    }

    #[test]
    fn a_clock_range_is_full_only_with_both_ends_in_order() {
        let range = |min, max| ClockRange { min, max };
        assert_eq!(range(Some(97), Some(1500)).full(), Some((97, 1500)));
        assert_eq!(range(Some(97), None).full(), None);
        assert_eq!(range(None, Some(1500)).full(), None);
        assert_eq!(range(Some(1500), Some(97)).full(), None);
    }

    #[test]
    fn device_clocks_info_names_the_gpu_and_a_refusal_is_the_daemons_message() {
        let dir = temp("clocks");
        let socket = dir.0.join("lactd.sock");
        let client = LactClient::new(&socket).with_timeout(Duration::from_secs(2));
        let server = answer_once(
            UnixListener::bind(&socket).unwrap(),
            r#"{"status":"error","data":{"description":"Clocks table not available","source":{"description":"No such file or directory","source":null}}}"#,
        );
        let error = client.device_clocks_info("x-0000:0d:00.0").unwrap_err();
        assert!(
            matches!(&error, LactError::Daemon(m) if m.starts_with("Clocks table not available"))
        );
        assert_eq!(
            server.join().unwrap(),
            json!({ "command": "device_clocks_info", "args": { "id": "x-0000:0d:00.0" } })
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
