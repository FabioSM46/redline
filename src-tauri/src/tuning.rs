//! The tuning service: what LACT can change on a GPU, and why not when it cannot.
//!
//! It detects LACT (installed, `lactd` running, socket permission, version), reads one GPU's
//! tunable state through `redline-core::lact`, and applies changes through `lactd`, always
//! pending the user's confirmation. Every request goes to the `lactd` socket from a blocking
//! thread, never from the main thread. Redline never writes to sysfs and never runs as root
//! (AGENTS.md, "GPU Rules").
//!
//! A missing tool is a disabled feature, not an error: every failure becomes
//! `available: false` with a sentence that says what to do, and a GPU LACT does not manage is
//! "not tunable", not an error. A LACT version other than [`VERIFIED_VERSION`] fills `warning` and
//! blocks nothing. Values LACT does not report are `null`, never zero.
//!
//! # Pending changes
//!
//! Verified against lactd 0.10.1 (`edit_gpu_config`, `wait_config_confirm` and
//! `confirm_pending_config` in `lact-daemon/src/server/handler.rs`): a setter applies the new
//! configuration at once and answers with `apply_settings_timer`, the seconds lactd waits for
//! `confirm_pending_config` (5 by default, `apply_settings_timer` in `/etc/lact/config.yaml`).
//! `confirm` keeps and saves the change; `revert`, or no answer before the timer, restores the
//! previous configuration — lactd does that by itself. lactd holds **one** pending change for the
//! whole daemon (the confirmation names no GPU) and refuses any setter while it exists.
//!
//! Redline mirrors that: [`Tuning::apply`] sends a setter, records the [`PendingChange`] with its
//! deadline and emits [`PENDING_EVENT`]. The change settles exactly once, with
//! [`SETTLED_EVENT`]: `confirmed` or `reverted` when the user answers ([`Tuning::settle`]),
//! `expired` when the deadline passes first. At the deadline Redline sends nothing: lactd has
//! already reverted, and a late `revert` could only hit a change made after it. Nothing in
//! Redline ever confirms on its own; quitting the app with a change pending sends `revert`.
//! Because lactd's pending change is global, a second setter is refused while any GPU has one.
//!
//! A profile switch is pending too, but lactd 0.10.1 does not hold it ([`profiles`]): Redline
//! keeps the previous profile and, on `revert`, at the deadline or when the app quits, switches
//! back to it itself.
//!
//! Overdrive is not a pending change: it is a kernel parameter, set by lactd for the next boot
//! after a confirmation handshake of its own ([`overdrive`]).

pub mod clocks;
pub mod overdrive;
pub mod profiles;

use redline_core::lact::{
    self, DeviceFlag, DeviceStats, FanControlMode, FanInfo, LactClient, LactError, PmfwInfo,
    VERIFIED_VERSION,
};
use redline_core::thumbnail::find_program;
use serde::Serialize;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{App, AppHandle, Emitter, Manager, Runtime, State};

pub mod fan;

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
pub const PENDING_HERE: &str = "A change to this GPU is still waiting for confirmation: confirm \
    or revert it first.";
pub const PENDING_ELSEWHERE: &str = "A change to another GPU is still waiting for confirmation: \
    LACT holds one pending change at a time, so confirm or revert it first.";
pub const PENDING_OUTSIDE: &str = "LACT has a change waiting for confirmation from another \
    program (such as the LACT app): confirm or revert it there first.";
pub const NOTHING_PENDING: &str = "No change to this GPU is waiting for confirmation.";
pub const NO_POWER_RANGE: &str = "This GPU does not report the range of its power limit, so \
    Redline cannot check the value.";

/// Emitted when a change starts waiting for confirmation, with a [`PendingChange`].
pub const PENDING_EVENT: &str = "tuning-pending";
/// Emitted once per pending change when it settles, with a [`Settled`].
pub const SETTLED_EVENT: &str = "tuning-settled";

/// What `tuning_status` returns. `power`, `fan` and `clocks` are `null` when `available` is false.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TuningStatus {
    pub available: bool,
    /// Why tuning is not available, as a sentence that says what to do.
    pub unavailable_reason: Option<String>,
    /// A LACT version other than the verified one: shown, never a block.
    pub warning: Option<String>,
    /// The version `lactd` reports.
    pub lact_version: Option<String>,
    /// The `amdgpu` overdrive bit, as LACT reads it; `null` when unknown. It is the running
    /// kernel's value: after enabling overdrive it stays false until the reboot.
    pub overdrive_enabled: Option<bool>,
    /// Overdrive was enabled or disabled through Redline in this session: the change takes
    /// effect at the next boot.
    pub reboot_pending: bool,
    pub power: Option<PowerLimit>,
    pub fan: Option<FanStatus>,
    /// The clock and voltage controls of this GPU; `null` when the feature does not apply or
    /// lactd did not answer that request ([`clocks`]).
    pub clocks: Option<clocks::ClocksStatus>,
    /// The active LACT profile; `null` is LACT's default profile (or an unreadable list).
    pub profile: Option<String>,
    /// The change to this GPU waiting for confirmation, if there is one.
    pub pending: Option<PendingChange>,
}

/// What a pending change touches. The later setters (fan, overdrive, profiles) add theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TuningChange {
    PowerLimit,
    Fan,
    /// Every GPU: the active LACT profile ([`profiles`]).
    Profile,
    /// Clock and voltage values, or their reset ([`clocks`]).
    Clocks,
}

/// A change lactd has applied and will revert unless the user confirms it: the payload of
/// [`PENDING_EVENT`] and `TuningStatus.pending`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingChange {
    pub gpu_id: String,
    pub what: TuningChange,
    /// When lactd reverts the change by itself, in milliseconds since the Unix epoch.
    pub deadline_ms: u64,
}

/// How a pending change ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The user confirmed it: lactd keeps and saves it.
    Confirmed,
    /// The user reverted it (or quit the app): lactd restored the previous values.
    Reverted,
    /// Nobody answered in time: lactd restored the previous values by itself.
    Expired,
}

/// The payload of [`SETTLED_EVENT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Settled {
    pub gpu_id: String,
    pub outcome: Outcome,
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// What a fixed speed or a curve must respect (`set_fan` checks exactly these); `null` when
    /// neither can be set.
    pub curve_rules: Option<fan::CurveRules>,
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
            reboot_pending: false,
            power: None,
            fan: None,
            clocks: None,
            profile: None,
            pending: None,
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
    /// Where the DRM cards are read (the firmware fan curve); tests pass a fake tree.
    drm: PathBuf,
    /// The file LACT writes to enable overdrive; tests pass a temporary one.
    modprobe_file: PathBuf,
}

impl Lact {
    pub fn new(client: LactClient, search_path: OsString) -> Self {
        Lact {
            client,
            search_path,
            drm: PathBuf::from(redline_core::sensors::DRM_ROOT),
            modprobe_file: PathBuf::from(lact::OVERDRIVE_MODPROBE_FILE),
        }
    }

    /// The same, reading the DRM cards from `drm` instead of sysfs.
    #[cfg(test)]
    pub fn with_drm_root(mut self, drm: impl Into<PathBuf>) -> Self {
        self.drm = drm.into();
        self
    }

    /// The same, looking for LACT's overdrive file at `file`.
    #[cfg(test)]
    pub fn with_modprobe_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.modprobe_file = file.into();
        self
    }

    /// The system's `lactd` socket and the process PATH.
    pub fn system() -> Self {
        Lact::new(
            LactClient::default(),
            std::env::var_os("PATH").unwrap_or_default(),
        )
    }

    /// The `lact` binary is on the search path.
    pub fn installed(&self) -> bool {
        find_program(LACT_BINARY, &self.search_path).is_some()
    }

    /// `lactd`'s `system_info`: answers only when the daemon runs and the user may use it.
    pub fn system_info(&self) -> lact::LactResult<lact::SystemInfo> {
        self.client.system_info()
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

    /// The sentence for a failed setter: a refusal from lactd is its own message.
    fn refusal(&self, error: &LactError) -> String {
        match error {
            LactError::Daemon(message) if message.contains("unconfirmed configuration change") => {
                PENDING_OUTSIDE.to_owned()
            }
            LactError::Daemon(message) => format!("LACT did not apply the change: {message}"),
            other => self.reason(other),
        }
    }

    /// The LACT id of the GPU with Redline id `gpu_id`.
    fn device_id(&self, gpu_id: &str) -> Result<String, String> {
        let devices = self
            .client
            .list_devices()
            .map_err(|error| self.reason(&error))?;
        lact::find_device(&devices, gpu_id)
            .map(|device| device.id.clone())
            .ok_or_else(|| NOT_MANAGED.to_owned())
    }

    /// Sends `set_power_cap` after checking `watts` against the range LACT reports.
    fn set_power_cap(&self, lact_id: &str, watts: Option<f64>) -> Result<u64, String> {
        if let Some(watts) = watts {
            let stats = self
                .client
                .device_stats(lact_id)
                .map_err(|error| self.reason(&error))?;
            check_power_limit(watts, &power_limit(&stats))?;
        }
        self.client
            .set_power_cap(lact_id, watts)
            .map_err(|error| self.refusal(&error))
    }

    /// The tuning state of the GPU with Redline id `gpu_id` (its PCI slot). Blocking: up to
    /// six requests to `lactd`, each bounded by the client's timeout.
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
        if let Some(fan) = status.fan.as_mut() {
            self.add_fan_capabilities(fan, gpu_id, &info, &stats);
        }
        status.clocks = clocks::clocks_status(
            self.client.device_clocks_info(&device.id),
            status.overdrive_enabled,
        );
        status.profile = self
            .client
            .list_profiles()
            .ok()
            .and_then(|p| p.current_profile);
        status
    }
}

/// `watts` inside the range `tuning_status` reports, or the sentence that says why not.
pub fn check_power_limit(watts: f64, limit: &PowerLimit) -> Result<(), String> {
    if !watts.is_finite() {
        return Err("The power limit must be a number of watts.".to_owned());
    }
    let (Some(min), Some(max)) = (limit.min_w, limit.max_w) else {
        return Err(NO_POWER_RANGE.to_owned());
    };
    if watts < min || watts > max {
        return Err(format!(
            "The power limit must be between {min} W and {max} W."
        ));
    }
    Ok(())
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
        curve_rules: None,
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

/// lactd 0.10.1's answers to `confirm_pending_config` when no change is pending any more:
/// "No pending config changes", or "Could not confirm config" when its own timer settled the
/// change a moment earlier.
fn nothing_pending_in_lactd(message: &str) -> bool {
    message.contains("No pending config changes") || message.contains("Could not confirm config")
}

/// Where the service reports: the app's webviews, or a test's collector.
pub trait Events: Send + Sync + 'static {
    fn pending(&self, change: &PendingChange);
    fn settled(&self, settled: &Settled);
    /// The profiles changed (one was created, activated, restored or deleted).
    fn profiles(&self, _profiles: &profiles::Profiles) {}
}

struct AppEvents<R: Runtime>(AppHandle<R>);

impl<R: Runtime> Events for AppEvents<R> {
    fn pending(&self, change: &PendingChange) {
        if let Err(error) = self.0.emit(PENDING_EVENT, change) {
            eprintln!("redline: could not send {PENDING_EVENT} ({error})");
        }
    }

    fn settled(&self, settled: &Settled) {
        if let Err(error) = self.0.emit(SETTLED_EVENT, settled) {
            eprintln!("redline: could not send {SETTLED_EVENT} ({error})");
        }
    }

    fn profiles(&self, profiles: &profiles::Profiles) {
        if let Err(error) = self.0.emit(profiles::PROFILE_EVENT, profiles) {
            eprintln!(
                "redline: could not send {} ({error})",
                profiles::PROFILE_EVENT
            );
        }
    }
}

/// A deadline timer's work.
pub type Job = Box<dyn FnOnce() + Send>;

/// The time and the deadline timer. Tests drive both by hand.
pub trait Clock: Send + Sync + 'static {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
    /// Runs `job` once, `delay_ms` from now, off the caller's thread.
    fn after(&self, delay_ms: u64, job: Job);
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn after(&self, delay_ms: u64, job: Job) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(delay_ms));
            job();
        });
    }
}

struct Pending {
    change: PendingChange,
    /// Tells this change's deadline timer apart from a later change's.
    serial: u64,
    /// How Redline undoes a change lactd does not hold as pending (a profile switch); `None`
    /// for lactd's own pending changes.
    undo: Option<profiles::Undo>,
}

#[derive(Default)]
struct PendingState {
    pending: Option<Pending>,
    serial: u64,
}

struct Inner {
    lact: Lact,
    clock: Arc<dyn Clock>,
    events: Arc<dyn Events>,
    /// Held for the whole of a setter, a confirmation or an expiry, `lactd` requests
    /// included: one at a time, in order.
    control: Mutex<()>,
    /// Held only to read or write the pending change, so `tuning_status` never waits on lactd.
    state: Mutex<PendingState>,
    /// The overdrive handshake; independent of the pending change.
    overdrive: overdrive::Overdrive,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The tuning service, shared as Tauri state. Cheap to clone.
#[derive(Clone)]
pub struct Tuning {
    inner: Arc<Inner>,
}

impl Tuning {
    pub fn new(lact: Lact, clock: Arc<dyn Clock>, events: Arc<dyn Events>) -> Self {
        Tuning {
            inner: Arc::new(Inner {
                lact,
                clock,
                events,
                control: Mutex::new(()),
                state: Mutex::new(PendingState::default()),
                overdrive: overdrive::Overdrive::default(),
            }),
        }
    }

    /// Where LACT is reached and looked for (the dependency status reads it too).
    pub fn lact(&self) -> &Lact {
        &self.inner.lact
    }

    /// `Lact::status` plus the change pending on this GPU. Blocking.
    pub fn status(&self, gpu_id: &str) -> TuningStatus {
        let mut status = self.inner.lact.status(gpu_id);
        status.pending = self.pending(gpu_id);
        status.reboot_pending = self.inner.overdrive.reboot_pending();
        status
    }

    /// The change waiting for confirmation on `gpu_id`, if there is one.
    pub fn pending(&self, gpu_id: &str) -> Option<PendingChange> {
        lock(&self.inner.state)
            .pending
            .as_ref()
            .filter(|pending| pending.change.gpu_id == gpu_id)
            .map(|pending| pending.change.clone())
    }

    /// Applies one change to `gpu_id` through lactd and makes it pending. `send` gets the LACT
    /// device id, sends the setter and returns lactd's answer: the seconds before it reverts.
    /// Refused while any change is pending. Blocking.
    pub fn apply(
        &self,
        gpu_id: &str,
        what: TuningChange,
        send: impl FnOnce(&Lact, &str) -> Result<u64, String>,
    ) -> Result<PendingChange, String> {
        self.apply_checked(gpu_id, what, false, send)
    }

    /// [`Tuning::apply`] for a change that may need overdrive at the next boot, refused once
    /// overdrive is being turned off. The check is under the control lock, where disabling reads
    /// what depends on overdrive: a change lands before that read or is refused.
    fn apply_needing_overdrive(
        &self,
        gpu_id: &str,
        what: TuningChange,
        send: impl FnOnce(&Lact, &str) -> Result<u64, String>,
    ) -> Result<PendingChange, String> {
        self.apply_checked(gpu_id, what, true, send)
    }

    fn apply_checked(
        &self,
        gpu_id: &str,
        what: TuningChange,
        needs_overdrive: bool,
        send: impl FnOnce(&Lact, &str) -> Result<u64, String>,
    ) -> Result<PendingChange, String> {
        let _control = lock(&self.inner.control);
        self.refuse_if_pending(gpu_id)?;
        if needs_overdrive {
            self.inner.overdrive.refuse_while_disabling()?;
        }
        let lact = &self.inner.lact;
        let seconds = send(lact, &lact.device_id(gpu_id)?)?;
        Ok(self.begin_pending(gpu_id, what, seconds.saturating_mul(1000), None))
    }

    /// The sentence that refuses a change while one is pending (on any GPU). Called with the
    /// control lock held.
    fn refuse_if_pending(&self, gpu_id: &str) -> Result<(), String> {
        self.expire_overdue();
        match &lock(&self.inner.state).pending {
            Some(pending) if pending.change.gpu_id == gpu_id => Err(PENDING_HERE.to_owned()),
            Some(_) => Err(PENDING_ELSEWHERE.to_owned()),
            None => Ok(()),
        }
    }

    /// Records an applied change as pending for `delay_ms`, reports it and starts its deadline
    /// timer. Called with the control lock held.
    fn begin_pending(
        &self,
        gpu_id: &str,
        what: TuningChange,
        delay_ms: u64,
        undo: Option<profiles::Undo>,
    ) -> PendingChange {
        let change = PendingChange {
            gpu_id: gpu_id.to_owned(),
            what,
            deadline_ms: self.inner.clock.now_ms().saturating_add(delay_ms),
        };
        let serial = {
            let mut state = lock(&self.inner.state);
            state.serial += 1;
            state.pending = Some(Pending {
                change: change.clone(),
                serial: state.serial,
                undo,
            });
            state.serial
        };
        self.inner.events.pending(&change);
        self.schedule_expiry(serial, delay_ms);
        change
    }

    /// The power limit of `gpu_id` in watts (`None` = the GPU's default), checked against the
    /// range LACT reports. Blocking.
    pub fn set_power_limit(
        &self,
        gpu_id: &str,
        watts: Option<f64>,
    ) -> Result<PendingChange, String> {
        self.apply(gpu_id, TuningChange::PowerLimit, |lact, id| {
            lact.set_power_cap(id, watts)
        })
    }

    /// The user's answer to the change pending on `gpu_id`: `confirm` keeps it, otherwise lactd
    /// restores the previous values. `expired` when lactd had already reverted it. An error
    /// (nothing pending, lactd unreachable) leaves the pending change as it was. Blocking.
    pub fn settle(&self, gpu_id: &str, confirm: bool) -> Result<Outcome, String> {
        let _control = lock(&self.inner.control);
        // An answer at or after the deadline is late, not lost: it reports `expired`.
        if self.expire_overdue().as_deref() == Some(gpu_id) {
            return Ok(Outcome::Expired);
        }
        let (serial, undo) = match &lock(&self.inner.state).pending {
            Some(pending) if pending.change.gpu_id == gpu_id => {
                (pending.serial, pending.undo.clone())
            }
            _ => return Err(NOTHING_PENDING.to_owned()),
        };
        if let Some(undo) = undo {
            // lactd holds nothing: confirming keeps what it already applied, reverting is
            // Redline's own request. A failed revert keeps the change pending.
            if !confirm {
                undo.run(self)?;
            }
            let outcome = if confirm {
                Outcome::Confirmed
            } else {
                Outcome::Reverted
            };
            self.finish(serial, outcome);
            return Ok(outcome);
        }
        let outcome = match self.inner.lact.client.confirm_pending(confirm) {
            Ok(()) if confirm => Outcome::Confirmed,
            Ok(()) => Outcome::Reverted,
            // The two 0.10.1 refusals that mean nothing is pending any more: its timer won.
            Err(LactError::Daemon(message)) if nothing_pending_in_lactd(&message) => {
                Outcome::Expired
            }
            // Anything else (lactd unreachable, a refusal Redline does not know) keeps the
            // change pending: its deadline still settles it.
            Err(error) => return Err(self.inner.lact.reason(&error)),
        };
        self.finish(serial, outcome);
        Ok(outcome)
    }

    /// When the app quits: a change still pending is reverted, never left to chance.
    pub fn shutdown(&self) {
        let _control = lock(&self.inner.control);
        self.expire_overdue();
        let Some((serial, undo)) = lock(&self.inner.state)
            .pending
            .as_ref()
            .map(|p| (p.serial, p.undo.clone()))
        else {
            return;
        };
        let reverted = match undo {
            // No window is left for `tuning-profile`: one request, not two.
            Some(undo) => undo.restore(self),
            None => self
                .inner
                .lact
                .client
                .confirm_pending(false)
                .map_err(|error| error.to_string()),
        };
        if let Err(error) = reverted {
            eprintln!("redline: could not revert the pending tuning change ({error})");
        }
        self.finish(serial, Outcome::Reverted);
    }

    /// Settles the pending change `serial` (if it is still the pending one) and reports it.
    fn finish(&self, serial: u64, outcome: Outcome) {
        let settled = {
            let mut state = lock(&self.inner.state);
            match state.pending.take_if(|pending| pending.serial == serial) {
                Some(pending) => Settled {
                    gpu_id: pending.change.gpu_id,
                    outcome,
                },
                None => return,
            }
        };
        self.inner.events.settled(&settled);
    }

    /// A pending change past its deadline is `expired`: lactd has already reverted it (or
    /// Redline undoes it now, for a change lactd does not hold). Returns the GPU whose change it
    /// settled, if any.
    fn expire_overdue(&self) -> Option<String> {
        let now = self.inner.clock.now_ms();
        let (serial, gpu_id) = lock(&self.inner.state)
            .pending
            .as_ref()
            .filter(|pending| now >= pending.change.deadline_ms)
            .map(|pending| (pending.serial, pending.change.gpu_id.clone()))?;
        self.expire(serial);
        Some(gpu_id)
    }

    /// Settles the pending change `serial` as `expired`, first undoing it when lactd does not
    /// hold it. Called with the control lock held.
    fn expire(&self, serial: u64) {
        let undo = lock(&self.inner.state)
            .pending
            .as_ref()
            .filter(|pending| pending.serial == serial)
            .and_then(|pending| pending.undo.clone());
        if let Some(undo) = undo
            && let Err(error) = undo.run(self)
        {
            eprintln!("redline: could not undo the expired tuning change ({error})");
        }
        self.finish(serial, Outcome::Expired);
    }

    fn schedule_expiry(&self, serial: u64, delay_ms: u64) {
        let tuning = self.clone();
        self.inner.clock.after(
            delay_ms,
            Box::new(move || {
                // After any confirmation already in flight: it may still win.
                let _control = lock(&tuning.inner.control);
                let deadline = lock(&tuning.inner.state)
                    .pending
                    .as_ref()
                    .filter(|pending| pending.serial == serial)
                    .map(|pending| pending.change.deadline_ms);
                let Some(deadline) = deadline else { return };
                let now = tuning.inner.clock.now_ms();
                if now >= deadline {
                    tuning.expire(serial);
                } else {
                    // Woken early (the wall clock moved): wait for the rest.
                    tuning.schedule_expiry(serial, deadline - now);
                }
            }),
        );
    }
}

/// Registers the service. Nothing is contacted until the first command.
pub fn init(app: &mut App) {
    let events = Arc::new(AppEvents(app.handle().clone()));
    app.manage(Tuning::new(Lact::system(), Arc::new(SystemClock), events));
}

/// When the app quits: reverts a change still waiting for confirmation.
pub fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    if let Some(tuning) = app.try_state::<Tuning>() {
        tuning.shutdown();
    }
}

/// Runs a blocking tuning job off the main thread.
async fn blocking<T: Send + 'static>(
    tuning: &State<'_, Tuning>,
    job: impl FnOnce(&Tuning) -> T + Send + 'static,
) -> Result<T, String> {
    let tuning = tuning.inner().clone();
    tauri::async_runtime::spawn_blocking(move || job(&tuning))
        .await
        .map_err(|error| format!("The tuning service did not answer: {error}"))
}

/// The tuning state of the GPU with id `gpu_id` (its PCI slot). Never an error for a missing
/// LACT or an unmanaged GPU: those are `available: false` with the reason.
#[tauri::command]
pub async fn tuning_status(
    gpu_id: String,
    tuning: State<'_, Tuning>,
) -> Result<TuningStatus, String> {
    blocking(&tuning, move |tuning| tuning.status(&gpu_id)).await
}

/// Sets the power limit of `gpu_id` in watts (`null` = the GPU's default). The change is
/// pending: the user must confirm it with `tuning_confirm` before its deadline.
#[tauri::command]
pub async fn set_power_limit(
    gpu_id: String,
    watts: Option<f64>,
    tuning: State<'_, Tuning>,
) -> Result<PendingChange, String> {
    blocking(&tuning, move |tuning| {
        tuning.set_power_limit(&gpu_id, watts)
    })
    .await?
}

/// Keeps the change pending on `gpu_id`. `expired` when it came too late.
#[tauri::command]
pub async fn tuning_confirm(gpu_id: String, tuning: State<'_, Tuning>) -> Result<Outcome, String> {
    blocking(&tuning, move |tuning| tuning.settle(&gpu_id, true)).await?
}

/// Restores the values the change pending on `gpu_id` replaced.
#[tauri::command]
pub async fn tuning_revert(gpu_id: String, tuning: State<'_, Tuning>) -> Result<Outcome, String> {
    blocking(&tuning, move |tuning| tuning.settle(&gpu_id, false)).await?
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

    pub(super) const TIMEOUT: Duration = Duration::from_secs(2);
    const GPU_A: &str = "1002:164e-1043:8877-0000:0c:00.0";
    const GPU_B: &str = "1002:7550-1043:0643-0000:0d:00.0";

    pub(super) type Handler = dyn Fn(&str, &Value) -> Value + Send + Sync;

    /// A fake `lactd`: answers each request line with `ok` and the handler's data, or with
    /// `error` when the handler returns `{"error": "<message>"}`. Stopped (and joined) on drop.
    pub(super) struct FakeLactd {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl FakeLactd {
        pub(super) fn start(socket: &Path, handler: Box<Handler>) -> Self {
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

    pub(super) struct Setup {
        dir: TempDir,
    }

    impl Setup {
        pub(super) fn new(name: &str) -> Self {
            Setup {
                dir: TempDir::new("tuning", name),
            }
        }
        pub(super) fn socket(&self) -> PathBuf {
            self.dir.join("lactd.sock")
        }
        /// A PATH with an executable `lact` in it, or an empty one.
        pub(super) fn lact(&self, installed: bool) -> Lact {
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
        assert_eq!(json["clocks"], Value::Null);
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

    /// A clock that moves only when the test says so, and runs the timers that fall due.
    #[derive(Default)]
    pub(super) struct ManualClock {
        now: Mutex<u64>,
        timers: Mutex<Vec<(u64, Job)>>,
    }

    impl ManualClock {
        pub(super) fn advance(&self, ms: u64) {
            let now = {
                let mut now = self.now.lock().unwrap();
                *now += ms;
                *now
            };
            let due: Vec<_> = {
                let mut timers = self.timers.lock().unwrap();
                let (due, later) = timers.drain(..).partition(|(at, _)| *at <= now);
                *timers = later;
                due
            };
            for (_, job) in due {
                job();
            }
        }
    }

    impl Clock for ManualClock {
        fn now_ms(&self) -> u64 {
            *self.now.lock().unwrap()
        }
        fn after(&self, delay_ms: u64, job: Job) {
            let at = self.now_ms() + delay_ms;
            self.timers.lock().unwrap().push((at, job));
        }
    }

    #[derive(Default)]
    pub(super) struct Collected(Mutex<Vec<Value>>);

    impl Events for Collected {
        fn pending(&self, change: &PendingChange) {
            let mut event = serde_json::to_value(change).unwrap();
            event["event"] = json!(PENDING_EVENT);
            self.0.lock().unwrap().push(event);
        }
        fn settled(&self, settled: &Settled) {
            let mut event = serde_json::to_value(settled).unwrap();
            event["event"] = json!(SETTLED_EVENT);
            self.0.lock().unwrap().push(event);
        }
        fn profiles(&self, profiles: &profiles::Profiles) {
            let mut event = serde_json::to_value(profiles).unwrap();
            event["event"] = json!(profiles::PROFILE_EVENT);
            self.0.lock().unwrap().push(event);
        }
    }

    impl Collected {
        pub(super) fn take(&self) -> Vec<Value> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    /// What the fake lactd holds: the applied cap, the one to restore, and every setter or
    /// confirmation it received.
    #[derive(Default)]
    struct Daemon {
        cap: f64,
        previous: Option<f64>,
        log: Vec<String>,
    }

    /// lactd 0.10.1's pending-change rules (`edit_gpu_config`, `confirm_pending_config`), for
    /// both GPUs of `two_gpus`. Its own revert timer is not modelled: the tests move Redline's
    /// clock, and lactd answers as it would after its timer when `daemon.previous` is cleared.
    fn pending_lactd(daemon: Arc<Mutex<Daemon>>) -> Box<Handler> {
        let read = two_gpus(VERIFIED_VERSION);
        Box::new(move |command, args| {
            let mut daemon = daemon.lock().unwrap();
            match command {
                "set_power_cap" => {
                    daemon
                        .log
                        .push(format!("set_power_cap {} {}", args["id"], args["cap"]));
                    if daemon.previous.is_some() {
                        return json!({ "error": "There is an unconfirmed configuration change pending" });
                    }
                    daemon.previous = Some(daemon.cap);
                    daemon.cap = args["cap"].as_f64().unwrap_or(317.0);
                    json!(5)
                }
                "confirm_pending_config" => {
                    let answer = args["command"].as_str().unwrap().to_owned();
                    daemon.log.push(answer.clone());
                    match daemon.previous.take() {
                        None => json!({ "error": "No pending config changes" }),
                        Some(previous) => {
                            if answer == "revert" {
                                daemon.cap = previous;
                            }
                            Value::Null
                        }
                    }
                }
                "device_stats" if args["id"] == GPU_B => stats_for(daemon.cap),
                _ => read(command, args),
            }
        })
    }

    struct Service {
        setup: Setup,
        daemon: Arc<Mutex<Daemon>>,
        clock: Arc<ManualClock>,
        events: Arc<Collected>,
        tuning: Tuning,
        _lactd: FakeLactd,
    }

    const SLOT_A: &str = "0000:0c:00.0";
    const SLOT_B: &str = "0000:0d:00.0";

    impl Service {
        fn start(name: &str) -> Self {
            let setup = Setup::new(name);
            let daemon = Arc::new(Mutex::new(Daemon {
                cap: 317.0,
                ..Daemon::default()
            }));
            let lactd = FakeLactd::start(&setup.socket(), pending_lactd(daemon.clone()));
            let clock = Arc::new(ManualClock::default());
            let events = Arc::new(Collected::default());
            let tuning = Tuning::new(setup.lact(true), clock.clone(), events.clone());
            Service {
                setup,
                daemon,
                clock,
                events,
                tuning,
                _lactd: lactd,
            }
        }

        fn cap(&self) -> f64 {
            self.daemon.lock().unwrap().cap
        }

        fn log(&self) -> Vec<String> {
            std::mem::take(&mut self.daemon.lock().unwrap().log)
        }

        /// lactd's own timer fired: it restored the previous configuration.
        fn lactd_reverts(&self) {
            let mut daemon = self.daemon.lock().unwrap();
            if let Some(previous) = daemon.previous.take() {
                daemon.cap = previous;
            }
        }
    }

    fn pending_event(slot: &str, deadline_ms: u64) -> Value {
        json!({
            "event": PENDING_EVENT, "gpu_id": slot, "what": "power_limit",
            "deadline_ms": deadline_ms
        })
    }

    fn settled_event(slot: &str, outcome: &str) -> Value {
        json!({ "event": SETTLED_EVENT, "gpu_id": slot, "outcome": outcome })
    }

    #[test]
    fn a_confirmed_power_limit_stays() {
        let service = Service::start("confirm");
        service.clock.advance(1_000);
        let change = service.tuning.set_power_limit(SLOT_B, Some(250.0)).unwrap();
        assert_eq!(
            change,
            PendingChange {
                gpu_id: SLOT_B.into(),
                what: TuningChange::PowerLimit,
                // lactd answered 5 seconds.
                deadline_ms: 6_000,
            }
        );
        assert_eq!(service.cap(), 250.0);
        assert_eq!(service.events.take(), vec![pending_event(SLOT_B, 6_000)]);

        // The status reports it, on this GPU only.
        assert_eq!(service.tuning.status(SLOT_B).pending, Some(change));
        assert_eq!(service.tuning.status(SLOT_A).pending, None);

        service.clock.advance(2_000);
        assert_eq!(service.tuning.settle(SLOT_B, true), Ok(Outcome::Confirmed));
        assert_eq!(service.cap(), 250.0);
        assert_eq!(
            service.log(),
            vec![format!("set_power_cap \"{GPU_B}\" 250.0"), "confirm".into()]
        );
        assert_eq!(
            service.events.take(),
            vec![settled_event(SLOT_B, "confirmed")]
        );
        assert_eq!(service.tuning.status(SLOT_B).pending, None);

        // Its deadline timer finds nothing left to do.
        service.clock.advance(10_000);
        assert_eq!(service.events.take(), Vec::<Value>::new());
    }

    #[test]
    fn a_reverted_power_limit_restores_the_previous_one() {
        let service = Service::start("revert");
        service.tuning.set_power_limit(SLOT_B, Some(300.0)).unwrap();
        service.events.take();
        assert_eq!(service.tuning.settle(SLOT_B, false), Ok(Outcome::Reverted));
        assert_eq!(service.cap(), 317.0);
        assert_eq!(service.log().last().map(String::as_str), Some("revert"));
        assert_eq!(
            service.events.take(),
            vec![settled_event(SLOT_B, "reverted")]
        );
        // Nothing left to answer.
        assert_eq!(
            service.tuning.settle(SLOT_B, true),
            Err(NOTHING_PENDING.to_owned())
        );
    }

    #[test]
    fn without_an_answer_it_expires_and_redline_sends_nothing() {
        let service = Service::start("expire");
        service.tuning.set_power_limit(SLOT_B, None).unwrap();
        service.log();
        service.events.take();

        service.clock.advance(4_999);
        assert_eq!(service.events.take(), Vec::<Value>::new());
        service.lactd_reverts();
        service.clock.advance(1);
        assert_eq!(
            service.events.take(),
            vec![settled_event(SLOT_B, "expired")]
        );
        assert_eq!(service.tuning.status(SLOT_B).pending, None);
        // lactd reverted by itself: a late `revert` could only hit a newer change.
        assert_eq!(service.log(), Vec::<String>::new());
        assert_eq!(service.cap(), 317.0);
    }

    #[test]
    fn a_confirmation_after_lactds_own_revert_reports_expired() {
        let service = Service::start("late");
        service.tuning.set_power_limit(SLOT_B, Some(250.0)).unwrap();
        service.events.take();
        // lactd's timer started before Redline's deadline and has already reverted.
        service.lactd_reverts();
        assert_eq!(service.tuning.settle(SLOT_B, true), Ok(Outcome::Expired));
        assert_eq!(service.cap(), 317.0);
        assert_eq!(
            service.events.take(),
            vec![settled_event(SLOT_B, "expired")]
        );
    }

    #[test]
    fn an_answer_at_the_deadline_reports_expired_not_an_error() {
        let service = Service::start("deadline");
        service.tuning.set_power_limit(SLOT_B, Some(250.0)).unwrap();
        service.events.take();
        service.log();
        service.lactd_reverts();
        // The clock reaches the deadline before the timer runs: the answer arrives first.
        *service.clock.now.lock().unwrap() = 5_000;
        assert_eq!(service.tuning.settle(SLOT_B, true), Ok(Outcome::Expired));
        assert_eq!(
            service.events.take(),
            vec![settled_event(SLOT_B, "expired")]
        );
        // Nothing was sent to lactd, and the other GPU never had a change.
        assert_eq!(service.log(), Vec::<String>::new());
        assert_eq!(
            service.tuning.settle(SLOT_A, false),
            Err(NOTHING_PENDING.to_owned())
        );
    }

    #[test]
    fn an_unknown_lactd_refusal_keeps_the_change_pending() {
        let setup = Setup::new("refusal");
        let _lactd = FakeLactd::start(
            &setup.socket(),
            Box::new(|command, args| match command {
                "set_power_cap" => json!(5),
                "confirm_pending_config" => json!({ "error": "Could not save the config" }),
                _ => two_gpus(VERIFIED_VERSION)(command, args),
            }),
        );
        let clock = Arc::new(ManualClock::default());
        let events = Arc::new(Collected::default());
        let tuning = Tuning::new(setup.lact(true), clock.clone(), events.clone());
        tuning.set_power_limit(SLOT_B, None).unwrap();
        events.take();
        let error = tuning.settle(SLOT_B, true).unwrap_err();
        assert!(error.contains("Could not save the config"), "{error}");
        assert!(tuning.pending(SLOT_B).is_some());
        assert_eq!(events.take(), Vec::<Value>::new());
        clock.advance(5_000);
        assert_eq!(events.take(), vec![settled_event(SLOT_B, "expired")]);
    }

    #[test]
    fn a_second_setter_is_refused_while_a_change_is_pending() {
        let service = Service::start("second");
        service.tuning.set_power_limit(SLOT_B, Some(250.0)).unwrap();
        service.log();
        assert_eq!(
            service.tuning.set_power_limit(SLOT_B, Some(260.0)),
            Err(PENDING_HERE.to_owned())
        );
        // lactd holds one pending change for the whole daemon: another GPU waits too.
        assert_eq!(
            service.tuning.set_power_limit(SLOT_A, Some(15.0)),
            Err(PENDING_ELSEWHERE.to_owned())
        );
        // Neither reached lactd, and the first change is untouched.
        assert_eq!(service.log(), Vec::<String>::new());
        assert_eq!(service.cap(), 250.0);
        // Only the GPU with the pending change can answer it.
        assert_eq!(
            service.tuning.settle(SLOT_A, true),
            Err(NOTHING_PENDING.to_owned())
        );

        // Once it settles, a new change is accepted.
        service.lactd_reverts();
        service.clock.advance(5_000);
        assert!(service.tuning.set_power_limit(SLOT_B, Some(260.0)).is_ok());
    }

    #[test]
    fn a_change_pending_in_another_lact_client_is_named() {
        let service = Service::start("outside");
        service.daemon.lock().unwrap().previous = Some(317.0);
        assert_eq!(
            service.tuning.set_power_limit(SLOT_B, None),
            Err(PENDING_OUTSIDE.to_owned())
        );
        assert_eq!(service.tuning.status(SLOT_B).pending, None);
        assert_eq!(service.events.take(), Vec::<Value>::new());
    }

    #[test]
    fn watts_outside_the_reported_range_are_refused_before_lactd() {
        let service = Service::start("range");
        for watts in [220.9, 340.5, f64::NAN, f64::INFINITY] {
            let error = service
                .tuning
                .set_power_limit(SLOT_B, Some(watts))
                .unwrap_err();
            assert!(error.contains("power limit"), "{watts}: {error}");
        }
        assert_eq!(
            service.tuning.set_power_limit(SLOT_B, Some(400.0)),
            Err("The power limit must be between 221 W and 340 W.".to_owned())
        );
        assert_eq!(service.log(), Vec::<String>::new());
        assert_eq!(service.events.take(), Vec::<Value>::new());
        // The bounds themselves are allowed.
        assert!(service.tuning.set_power_limit(SLOT_B, Some(221.0)).is_ok());
    }

    #[test]
    fn a_power_limit_needs_a_known_range() {
        let limit = PowerLimit {
            current_w: Some(200.0),
            min_w: None,
            max_w: Some(300.0),
            default_w: None,
        };
        assert_eq!(
            check_power_limit(250.0, &limit),
            Err(NO_POWER_RANGE.to_owned())
        );
    }

    #[test]
    fn a_gpu_lact_does_not_manage_cannot_be_changed() {
        let service = Service::start("unmanaged");
        assert_eq!(
            service.tuning.set_power_limit("0000:03:00.0", None),
            Err(NOT_MANAGED.to_owned())
        );
        assert_eq!(service.events.take(), Vec::<Value>::new());
    }

    #[test]
    fn quitting_with_a_change_pending_reverts_it() {
        let service = Service::start("shutdown");
        service.tuning.set_power_limit(SLOT_B, Some(250.0)).unwrap();
        service.events.take();
        service.log();
        service.tuning.shutdown();
        assert_eq!(service.log(), vec!["revert".to_owned()]);
        assert_eq!(service.cap(), 317.0);
        assert_eq!(
            service.events.take(),
            vec![settled_event(SLOT_B, "reverted")]
        );

        // Nothing pending: quitting sends nothing.
        service.tuning.shutdown();
        assert_eq!(service.log(), Vec::<String>::new());
    }

    #[test]
    fn an_unreachable_lactd_leaves_the_change_pending() {
        let mut service = Service::start("unreachable");
        service.tuning.set_power_limit(SLOT_B, Some(250.0)).unwrap();
        service.events.take();
        // lactd stops: no listener, the socket file removed.
        drop(std::mem::replace(
            &mut service._lactd,
            FakeLactd::start(
                &service.setup.dir.join("other.sock"),
                two_gpus(VERIFIED_VERSION),
            ),
        ));
        fs::remove_file(service.setup.socket()).unwrap();
        assert_eq!(
            service.tuning.settle(SLOT_B, true),
            Err(NOT_RUNNING.to_owned())
        );
        assert!(service.tuning.pending(SLOT_B).is_some());
        assert_eq!(service.events.take(), Vec::<Value>::new());
        // Its deadline still settles it.
        service.clock.advance(5_000);
        assert_eq!(
            service.events.take(),
            vec![settled_event(SLOT_B, "expired")]
        );
    }

    #[test]
    fn the_version_notice_is_empty_only_for_the_verified_version() {
        assert_eq!(version_notice(VERIFIED_VERSION), None);
        assert!(version_notice("0.10.0").is_some());
    }

    /// A fake sysfs tree with GPU B (`<dir>/drm/card1 -> <dir>/devices/<slot>`) and, when given,
    /// its firmware fan curve.
    fn fake_drm(dir: &Path, fan_curve: Option<&str>) -> PathBuf {
        let device = dir.join("devices").join(SLOT_B);
        fs::create_dir_all(device.join("hwmon/hwmon3")).unwrap();
        fs::write(device.join("vendor"), "0x1002\n").unwrap();
        if let Some(curve) = fan_curve {
            fs::create_dir_all(device.join("gpu_od/fan_ctrl")).unwrap();
            fs::write(device.join("gpu_od/fan_ctrl/fan_curve"), curve).unwrap();
        }
        let card = dir.join("drm/card1");
        fs::create_dir_all(&card).unwrap();
        std::os::unix::fs::symlink(&device, card.join("device")).unwrap();
        dir.join("drm")
    }

    const FAN_CURVE: &str = "OD_FAN_CURVE:\n0: 0C 0%\n1: 0C 0%\n2: 0C 0%\n3: 0C 0%\n4: 0C 0%\n\
        OD_RANGE:\nFAN_CURVE(hotspot temp): 25C 100C\nFAN_CURVE(fan speed): 15% 100%\n";

    /// lactd 0.10.1 with GPU B's firmware fan, overdrive on or off; every `set_fan_control`
    /// lands in `requests`.
    fn fan_lactd(overdrive: bool, requests: Arc<Mutex<Vec<Value>>>) -> Box<Handler> {
        let read = two_gpus(VERIFIED_VERSION);
        Box::new(move |command, args| match command {
            "device_info" if overdrive => json!({
                "driver": "amdgpu", "flags": ["HasPmfw", "ConfigurableFanControl"]
            }),
            "device_stats" if args["id"] == GPU_B => {
                let mut stats = stats_for(317.0);
                if overdrive {
                    stats["fan"]["temperature_range"] = json!([25, 100]);
                    stats["fan"]["pmfw_info"] = json!({
                        "acoustic_limit": { "current": 2450, "allowed_range": [500, 3200] },
                        "zero_rpm_enable": true,
                        "zero_rpm_temperature": { "current": 50, "allowed_range": [25, 100] }
                    });
                }
                stats
            }
            "get_gpu_config" => json!({
                "fan_control_enabled": false,
                "pmfw_options": { "acoustic_limit": 2800 },
                "power_cap": 317.0
            }),
            "set_fan_control" => {
                requests.lock().unwrap().push(args.clone());
                json!(5)
            }
            "confirm_pending_config" => {
                requests.lock().unwrap().push(json!(args["command"]));
                Value::Null
            }
            _ => read(command, args),
        })
    }

    struct FanService {
        _setup: Setup,
        requests: Arc<Mutex<Vec<Value>>>,
        events: Arc<Collected>,
        tuning: Tuning,
        _lactd: FakeLactd,
    }

    impl FanService {
        fn start(name: &str, overdrive: bool) -> Self {
            let setup = Setup::new(name);
            let requests = Arc::new(Mutex::new(Vec::new()));
            let lactd = FakeLactd::start(&setup.socket(), fan_lactd(overdrive, requests.clone()));
            let drm = fake_drm(&setup.dir, overdrive.then_some(FAN_CURVE));
            let events = Arc::new(Collected::default());
            let tuning = Tuning::new(
                setup.lact(true).with_drm_root(drm),
                Arc::new(ManualClock::default()),
                events.clone(),
            );
            FanService {
                _setup: setup,
                requests,
                events,
                tuning,
                _lactd: lactd,
            }
        }

        fn requests(&self) -> Vec<Value> {
            std::mem::take(&mut *self.requests.lock().unwrap())
        }
    }

    fn fan_settings(value: Value) -> fan::FanSettings {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_fan_curve_goes_to_lactd_as_a_pending_change() {
        let service = FanService::start("fan-curve", true);

        // The status reports the rules the setter applies: five points, the firmware's ranges.
        let rules = service
            .tuning
            .status(SLOT_B)
            .fan
            .unwrap()
            .curve_rules
            .unwrap();
        assert_eq!(rules.points, Some(5));
        assert_eq!(
            rules.temperature,
            TemperatureRange {
                min_c: 25,
                max_c: 100
            }
        );
        assert_eq!(rules.speed.min_pct, 15);

        let settings = fan_settings(json!({
            "mode": "curve",
            "curve": [
                { "temperature_c": 85, "speed_pct": 100 },
                { "temperature_c": 40, "speed_pct": 20 },
                { "temperature_c": 50, "speed_pct": 30 },
                { "temperature_c": 60, "speed_pct": 50 },
                { "temperature_c": 70, "speed_pct": 75 }
            ],
            "pmfw": { "zero_rpm": false }
        }));
        let change = service.tuning.set_fan(SLOT_B, &settings).unwrap();
        assert_eq!(change.what, TuningChange::Fan);
        assert_eq!(change.deadline_ms, 5_000);
        assert_eq!(
            service.events.take(),
            vec![json!({
                "event": PENDING_EVENT, "gpu_id": SLOT_B, "what": "fan", "deadline_ms": 5_000
            })]
        );
        // LACT's own fan request, with the stored acoustic limit kept next to the new option.
        let request = service.requests().remove(0);
        assert_eq!(request["id"], GPU_B);
        assert_eq!(request["enabled"], true);
        assert_eq!(request["mode"], "curve");
        assert_eq!(request["static_speed"], Value::Null);
        let curve = request["curve"].as_object().unwrap();
        assert_eq!(
            curve.keys().collect::<Vec<_>>(),
            vec!["40", "50", "60", "70", "85"]
        );
        assert_eq!(curve["85"], 1.0);
        assert_eq!(request["pmfw"]["acoustic_limit"], 2800);
        assert_eq!(request["pmfw"]["zero_rpm"], false);

        // Confirmed like any other change.
        assert_eq!(service.tuning.settle(SLOT_B, true), Ok(Outcome::Confirmed));
        assert_eq!(service.requests(), vec![json!("confirm")]);
    }

    #[test]
    fn auto_hands_the_fan_back_to_the_firmware() {
        let service = FanService::start("fan-auto", true);
        service
            .tuning
            .set_fan(
                SLOT_B,
                &fan_settings(json!({ "mode": "auto", "pmfw": { "acoustic_limit": 3000 } })),
            )
            .unwrap();
        let request = service.requests().remove(0);
        assert_eq!(request["enabled"], false);
        assert_eq!(request["mode"], Value::Null);
        assert_eq!(request["pmfw"]["acoustic_limit"], 3000);
    }

    #[test]
    fn with_overdrive_off_the_fan_is_refused_before_lactd() {
        let service = FanService::start("fan-off", false);
        let fan = service.tuning.status(SLOT_B).fan.unwrap();
        assert!(!fan.configurable);
        assert_eq!(fan.curve_rules, None);
        for settings in [
            json!({ "mode": "static", "static_speed": 60 }),
            json!({ "mode": "auto", "pmfw": { "zero_rpm": false } }),
        ] {
            assert_eq!(
                service.tuning.set_fan(SLOT_B, &fan_settings(settings)),
                Err(fan::NEEDS_OVERDRIVE.to_owned())
            );
        }
        let error = service
            .tuning
            .set_fan(
                SLOT_B,
                &fan_settings(json!({ "mode": "curve", "curve": [
                    { "temperature_c": 40, "speed_pct": 30 }
                ] })),
            )
            .unwrap_err();
        assert_eq!(error, fan::NEEDS_OVERDRIVE);
        assert_eq!(service.requests(), Vec::<Value>::new());
        assert_eq!(service.events.take(), Vec::<Value>::new());
        assert_eq!(service.tuning.pending(SLOT_B), None);
    }

    #[test]
    fn a_wrong_curve_is_refused_with_the_problem_named() {
        let service = FanService::start("fan-bad", true);
        let error = service
            .tuning
            .set_fan(
                SLOT_B,
                &fan_settings(json!({ "mode": "curve", "curve": [
                    { "temperature_c": 40, "speed_pct": 30 },
                    { "temperature_c": 80, "speed_pct": 100 }
                ] })),
            )
            .unwrap_err();
        assert_eq!(
            error,
            "This GPU's firmware fan curve has exactly 5 points; this curve has 2."
        );
        assert_eq!(service.requests(), Vec::<Value>::new());
        assert_eq!(service.events.take(), Vec::<Value>::new());
    }
}
