//! Overdrive (`amdgpu.ppfeaturemask`) through LACT, on explicit confirmation (D11).
//!
//! Overdrive is a kernel parameter: RDNA3/4 need it for the firmware fan curve, older GPUs for
//! clocks and voltages. Redline never edits it. On the user's confirmation it asks `lactd`, which
//! runs as root, to do it: `enable_overdrive` / `disable_overdrive` (verified against
//! `lact-daemon/src/system.rs` at `v0.10.1`). lactd sets or clears the overdrive bit in the boot
//! configuration ([`ParamsConfigurator`]) and regenerates the initramfs; the running kernel
//! keeps its value until the next boot, so `overdrive_enabled` stays as it was and
//! `reboot_pending` turns true. lactd allows one enable or disable per daemon start ("already
//! toggled - please reboot").
//!
//! It is not a pending change: lactd has nothing to confirm or revert, so the confirmation
//! happens before the request. `prepare_*` reads how lactd would do it and returns an
//! [`OverdrivePlan`]: the steps to show the user and a single-use token. `enable_overdrive` /
//! `disable_overdrive` send the request only with the token of the latest plan for the same
//! action, before it expires. A call without it never reaches lactd.
//!
//! # What depends on overdrive
//!
//! Disabling must never leave a fan the firmware only takes through overdrive under a setting
//! nothing can apply, so [`Tuning::prepare_overdrive`] lists what depends on it on **every** AMD
//! GPU ([`OverdriveDependency`]) and a plan with a list has no token. `change_overdrive` reads the
//! list again right before the request and refuses if it is not empty, cannot be read, or a change
//! is pending: a pending reset that reverts would bring its setting back. Redline's setters stop
//! once a disable is under way ([`Overdrive::refuse_while_disabling`]), because the running kernel
//! keeps overdrive until the reboot and would accept them. Nothing is reset on the user's behalf:
//! each item is one pending change the user keeps or reverts.
//!
//! Verified at LACT `v0.10.1`:
//!
//! - `get_gpu_config` reads `Config::gpus`, which is the active profile's copy when a profile is
//!   active (`Profile.gpus`, the same `GpuConfig`) and the default configuration otherwise. The
//!   setters write through `gpus_mut` to the same place, so what is listed is what lactd applies,
//!   and "Back to default" under a profile changes that profile.
//! - A stored curve stays stored with overdrive off, and `apply_config` still runs at every start.
//!   `get_fan_curve` fails without overdrive, so `start_curve_fan_control` falls back to its own
//!   loop on `pwm1_enable`/`pwm1` (`start_curve_fan_control_task`) and a fixed speed to the same
//!   hwmon files, on a fan the firmware drives. A stored firmware limit makes `apply_config` fail
//!   at its `get_fan_*`, which skips the rest of that GPU's configuration, power cap included.
//! - With `fan_control_enabled: false` the stored curve is unused: `stop_fan_control(true)` leaves
//!   the firmware in charge. That is why a curve only counts while the fan control is on.
//! - A fan LACT drives through hwmon (`HasPmfw` absent) needs no overdrive, and neither do GPUs
//!   on another driver, whatever `get_gpu_config` holds for them.

use super::{Lact, Tuning, blocking, lock};
use redline_core::lact::{
    DeviceFlag, DeviceListEntry, FanControlMode, GpuConfig, LactError, OVERDRIVE_MODPROBE_FILE,
    ParamsConfigurator, PmfwOptions,
};
use serde::Serialize;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::ErrorKind;
use std::sync::Mutex;
use tauri::State;

/// How long a plan's token stays valid: the time to read the dialog and decide.
pub const TOKEN_TTL_MS: u64 = 5 * 60 * 1000;

pub const NOT_CONFIRMED: &str = "Overdrive was not changed: the change was not confirmed. Review \
    what will change and confirm it again.";
pub const CONFIRMATION_EXPIRED: &str = "Overdrive was not changed: the confirmation expired. \
    Review what will change and confirm it again.";
pub const BUSY: &str = "LACT is already changing overdrive: wait for it to finish.";
pub const REBOOT_PENDING: &str = "Overdrive was already changed since the last boot. Reboot to \
    apply it before changing it again.";
pub const ALREADY_ENABLED: &str = "Overdrive is already enabled.";
pub const NO_CONFIGURATOR: &str = "LACT cannot set kernel parameters on this system (for \
    example on NixOS). Add the overdrive bit (0x4000) to `amdgpu.ppfeaturemask` in your system \
    configuration and reboot.";
pub const NOT_ENABLED_BY_LACT: &str = "Overdrive was not enabled through LACT, so LACT cannot \
    disable it. Remove `amdgpu.ppfeaturemask` from where it was set and reboot.";
pub const DEPENDS_ON_IT: &str = "Overdrive was not changed: a fan or clock setting still depends \
    on it. Review the list and put each one back to default first.";
pub const PENDING_FIRST: &str = "Overdrive was not changed: a tuning change is still waiting for \
    an answer. Keep or revert it first.";
pub const DISABLE_UNDER_WAY: &str = "Overdrive is being turned off for the next boot, so nothing \
    that needs it can be set now. Reboot first.";
pub const SAVED_PROFILES: &str = "Saved profiles that are not active keep their values and do \
    not block this; a fan curve or clock value in one comes back if you activate it.";
pub const AUTO_SWITCH: &str = "LACT is switching profiles automatically, so one of them can \
    become active at any time.";
pub const TIMED_OUT: &str = "LACT did not answer in time: it may still be updating the boot \
    files. Redline sends no other overdrive request for ten minutes; after that, if LACT \
    finished, it will say a reboot is needed.";
pub const STILL_RUNNING: &str = "LACT may still be updating the boot files after a request that \
    did not answer in time. Wait ten minutes before trying again, or reboot and check whether \
    overdrive changed.";

/// How long no overdrive request is sent after one timed out. lactd 0.10.1 marks overdrive as
/// toggled only once its work is done (`OC_TOGGLED` in `lact-daemon/src/system.rs`), so a
/// request sent while the timed-out one still runs would start a second initramfs regeneration
/// next to the first. As long as the request's own timeout: by then lactd has had twice that.
pub const STALLED_COOLDOWN_MS: u64 = 10 * 60 * 1000;

/// What a plan does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverdriveAction {
    Enable,
    Disable,
}

/// What the user confirms: returned by `prepare_enable_overdrive` / `prepare_disable_overdrive`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverdrivePlan {
    pub action: OverdriveAction,
    /// What lactd will do, in order, one sentence each.
    pub steps: Vec<String>,
    /// Always true: the kernel reads the parameter at boot.
    pub reboot_required: bool,
    /// What stops a disable, on every GPU. Empty for an enable.
    pub dependencies: Vec<OverdriveDependency>,
    /// What happens to saved profiles that are not active, on a disable that has some.
    pub saved_profiles: Option<String>,
    /// Pass it to `enable_overdrive` / `disable_overdrive` once the user confirms. Single use.
    /// `null` while `dependencies` is not empty: such a plan cannot be confirmed.
    pub token: Option<String>,
    /// When the token stops being accepted, in milliseconds since the Unix epoch.
    pub expires_ms: u64,
}

/// What depends on overdrive, and so which way back it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverdriveDependencyKind {
    /// Clock or voltage values: `reset_clocks`.
    Clocks,
    /// A fan curve, a fixed speed or firmware fan options: `reset_fan`.
    Fan,
}

/// One setting that stops overdrive from being disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverdriveDependency {
    pub gpu_id: String,
    pub kind: OverdriveDependencyKind,
    /// The active LACT profile that holds it; `null` is LACT's default configuration.
    pub profile: Option<String>,
    /// The GPU, the setting and where it comes from.
    pub sentence: String,
}

/// What `enable_overdrive` / `disable_overdrive` return when lactd made the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OverdriveResult {
    pub reboot_required: bool,
}

struct Ticket {
    action: OverdriveAction,
    token: String,
    expires_ms: u64,
}

#[derive(Default)]
struct HandshakeState {
    /// The latest plan's token; a new plan replaces it.
    ticket: Option<Ticket>,
    /// A request is with lactd now.
    busy: bool,
    /// A disable reached lactd or may have: from then until the reboot nothing that needs
    /// overdrive may be set. Only an answer that proves nothing changed lifts it
    /// ([`proves_nothing_changed`]).
    disabling: bool,
    /// A request timed out: until then lactd may still be running it.
    stalled_until_ms: Option<u64>,
    reboot_pending: bool,
}

/// The overdrive handshake's state, inside the tuning service.
#[derive(Default)]
pub struct Overdrive {
    state: Mutex<HandshakeState>,
}

impl Overdrive {
    /// Overdrive was changed through Redline in this session and waits for a reboot.
    pub fn reboot_pending(&self) -> bool {
        lock(&self.state).reboot_pending
    }

    /// Refuses a change that could need overdrive at the next boot while a disable is under way.
    pub(super) fn refuse_while_disabling(&self) -> Result<(), String> {
        if lock(&self.state).disabling {
            Err(DISABLE_UNDER_WAY.to_owned())
        } else {
            Ok(())
        }
    }

    /// Refuses while a request runs (or may still run in lactd after a timeout) or a reboot is
    /// pending.
    fn check_idle(state: &HandshakeState, now_ms: u64) -> Result<(), String> {
        if state.busy {
            Err(BUSY.to_owned())
        } else if state.stalled_until_ms.is_some_and(|until| now_ms < until) {
            Err(STILL_RUNNING.to_owned())
        } else if state.reboot_pending {
            Err(REBOOT_PENDING.to_owned())
        } else {
            Ok(())
        }
    }
}

/// 128 random bits as hex, from the standard library's per-process random hash keys.
fn new_token() -> String {
    let half = || {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u8(0);
        hasher.finish()
    };
    format!("{:016x}{:016x}", half(), half())
}

/// What lactd 0.10.1 does for `action` with `configurator`, one sentence per step.
pub fn steps(
    action: OverdriveAction,
    configurator: Option<ParamsConfigurator>,
) -> Result<Vec<String>, String> {
    let enable = action == OverdriveAction::Enable;
    let mut steps = match configurator {
        None => return Err(NO_CONFIGURATOR.to_owned()),
        Some(ParamsConfigurator::Other(name)) => {
            return Err(format!(
                "Redline does not know how this LACT version sets kernel parameters ({name}), so \
                 it cannot show what would change. Use the LACT app to change overdrive."
            ));
        }
        Some(ParamsConfigurator::RpmOstree) => vec![if enable {
            "LACT adds the overdrive bit of `amdgpu.ppfeaturemask` to the kernel arguments with \
             `rpm-ostree kargs`."
                .to_owned()
        } else {
            "LACT removes `amdgpu.ppfeaturemask` from the kernel arguments with `rpm-ostree kargs`."
                .to_owned()
        }],
        Some(ParamsConfigurator::Modprobe(initramfs)) => {
            let mut steps = vec![if enable {
                format!(
                    "LACT writes {OVERDRIVE_MODPROBE_FILE}, which adds the overdrive bit to the \
                     `amdgpu.ppfeaturemask` kernel module option."
                )
            } else {
                format!("LACT deletes {OVERDRIVE_MODPROBE_FILE}, the file it wrote to enable it.")
            }];
            if let Some(initramfs) = initramfs {
                steps.push(match initramfs.command() {
                    Some(command) => {
                        format!("LACT regenerates the initramfs with `{command}`; this can take a few minutes.")
                    }
                    None => "LACT regenerates the initramfs; this can take a few minutes."
                        .to_owned(),
                });
            }
            steps
        }
    };
    steps.push(
        "The change takes effect after a reboot. Redline does not reboot the machine.".to_owned(),
    );
    Ok(steps)
}

/// Where a setting lives, for its sentence.
fn stored_in(profile: Option<&str>) -> String {
    match profile {
        Some(profile) => format!("from the active profile \"{profile}\""),
        None => "stored in LACT".to_owned(),
    }
}

/// The fan setting of `config` that needs overdrive on a firmware fan, in words.
fn fan_setting(config: &GpuConfig) -> Option<String> {
    let driven = config.fan_control_enabled.then(|| {
        match config.fan_control_settings.as_ref().map(|fan| &fan.mode) {
            Some(FanControlMode::Curve) => "a fan curve",
            Some(FanControlMode::Static) => "a fixed fan speed",
            _ => "a fan setting",
        }
    });
    let options = (config.pmfw_options != PmfwOptions::default()).then_some("firmware fan options");
    match (driven, options) {
        (Some(driven), Some(options)) => Some(format!("{driven} and {options}")),
        (Some(setting), None) | (None, Some(setting)) => Some(setting.to_owned()),
        (None, None) => None,
    }
}

/// What in `config`, the stored configuration of `device`, depends on overdrive. A firmware fan
/// follows the rule of [`super::fan`]: `HasPmfw`, with no list of models.
pub fn dependencies(
    device: &DeviceListEntry,
    has_pmfw: bool,
    config: &GpuConfig,
    profile: Option<&str>,
) -> Vec<OverdriveDependency> {
    let gpu = device.name.as_deref().unwrap_or(device.pci_slot());
    let item = |kind, setting: &str| OverdriveDependency {
        gpu_id: device.pci_slot().to_owned(),
        kind,
        profile: profile.map(str::to_owned),
        sentence: format!("{gpu} has {setting} {}.", stored_in(profile)),
    };
    let mut items = Vec::new();
    if !config.clocks.is_empty() {
        items.push(item(
            OverdriveDependencyKind::Clocks,
            "clock or voltage values",
        ));
    }
    if has_pmfw && let Some(setting) = fan_setting(config) {
        items.push(item(OverdriveDependencyKind::Fan, &setting));
    }
    items
}

/// The sentence for a failed `enable_overdrive` / `disable_overdrive`. The second value is true
/// when lactd says it already changed overdrive since it started (a reboot is pending).
///
/// The fragments are lactd 0.10.1's own messages (`lact-daemon/src/system.rs` at `v0.10.1`),
/// reproduced verbatim by the fake lactd in the tests: "Overdrive support was already toggled -
/// please reboot to apply the changes", "Overdrive mask already enabled", "Overclocking was not
/// enabled through LACT (file at … does not exist)", "Overdrive should be toggled through system
/// config on NixOS". lactd does not localise them. A rewording in another LACT version falls
/// back to its message as is, and that version is already reported in `TuningStatus.warning`.
fn refusal(lact: &Lact, error: &LactError) -> (String, bool) {
    match error {
        LactError::Daemon(message) if message.contains("already toggled") => {
            (REBOOT_PENDING.to_owned(), true)
        }
        LactError::Daemon(message) if message.contains("mask already enabled") => {
            (ALREADY_ENABLED.to_owned(), false)
        }
        LactError::Daemon(message) if message.contains("not enabled through LACT") => {
            (NOT_ENABLED_BY_LACT.to_owned(), false)
        }
        LactError::Daemon(message) if message.contains("NixOS") => {
            (NO_CONFIGURATOR.to_owned(), false)
        }
        LactError::Daemon(message) => {
            (format!("LACT could not change overdrive: {message}"), false)
        }
        LactError::Io(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            (TIMED_OUT.to_owned(), false)
        }
        other => (lact.reason(other), false),
    }
}

/// `error` proves a disable changed nothing, so the stop on setters can go. The request never
/// reached lactd, or lactd refused before touching anything: its two refusals that come first in
/// `disable_overdrive` at `v0.10.1`. Anything else keeps the stop. "Already toggled" means
/// overdrive changed since lactd started. An error from the initramfs comes after the modprobe
/// file is gone. A transport or decoding error may have followed a finished request.
fn proves_nothing_changed(error: &LactError) -> bool {
    match error {
        LactError::SocketMissing | LactError::NotRunning | LactError::PermissionDenied => true,
        LactError::Daemon(message) => {
            message.contains("not enabled through LACT") || message.contains("NixOS")
        }
        LactError::Io(_) | LactError::Protocol(_) => false,
    }
}

impl Lact {
    /// What depends on overdrive on every GPU, read from what lactd stores now, and the note on
    /// saved profiles. An answer that cannot be read is an error, never an empty list.
    fn overdrive_dependencies(&self) -> Result<(Vec<OverdriveDependency>, Option<String>), String> {
        let unreadable = |error: LactError| {
            format!(
                "Overdrive was not changed: Redline could not read what depends on it, so it \
                 cannot tell that your fans stay controlled. {}",
                self.reason(&error)
            )
        };
        let profiles = self.client.list_profiles().map_err(unreadable)?;
        let active = profiles.current_profile.as_deref();
        let mut items = Vec::new();
        for device in self.client.list_devices().map_err(unreadable)? {
            let info = self.client.device_info(&device.id).map_err(unreadable)?;
            // Overdrive is an `amdgpu` parameter. A device that names no driver is checked.
            if info
                .driver
                .as_deref()
                .is_some_and(|driver| driver != "amdgpu")
            {
                continue;
            }
            if let Some(config) = self.client.get_gpu_config(&device.id).map_err(unreadable)? {
                let has_pmfw = info.has(&DeviceFlag::HasPmfw);
                items.extend(dependencies(&device, has_pmfw, &config, active));
            }
        }
        let saved = profiles
            .profiles
            .iter()
            .any(|name| Some(name.as_str()) != active);
        let note = saved.then(|| {
            if profiles.auto_switch {
                format!("{SAVED_PROFILES} {AUTO_SWITCH}")
            } else {
                SAVED_PROFILES.to_owned()
            }
        });
        Ok((items, note))
    }
}

impl Tuning {
    /// Refuses a disable while any change is pending. Called with the control lock held.
    fn refuse_disable_while_pending(&self) -> Result<(), String> {
        self.expire_overdue();
        match lock(&self.inner.state).pending {
            Some(_) => Err(PENDING_FIRST.to_owned()),
            None => Ok(()),
        }
    }

    /// Reads how lactd would do `action` and returns the plan to confirm. A new plan replaces
    /// the previous one's token; a disable with something depending on overdrive gets none.
    /// Blocking.
    pub fn prepare_overdrive(&self, action: OverdriveAction) -> Result<OverdrivePlan, String> {
        let overdrive = &self.inner.overdrive;
        let now = self.inner.clock.now_ms();
        Overdrive::check_idle(&lock(&overdrive.state), now)?;
        let lact = &self.inner.lact;
        let system = lact
            .client
            .system_info()
            .map_err(|error| lact.reason(&error))?;
        if action == OverdriveAction::Enable && system.amdgpu_overdrive_enabled == Some(true) {
            return Err(ALREADY_ENABLED.to_owned());
        }
        // Disabling stays possible with the bit off: LACT may have enabled it in a session
        // that has not rebooted yet.
        let configurator = system.params_configurator();
        let steps = steps(action, configurator.clone())?;
        let (dependencies, saved_profiles) = if action == OverdriveAction::Disable {
            // lactd would refuse after the user put every setting back: say it first. Only a
            // file known to be missing counts; lactd stays the judge of the rest.
            if matches!(configurator, Some(ParamsConfigurator::Modprobe(_)))
                && lact.modprobe_file.try_exists().is_ok_and(|found| !found)
            {
                return Err(NOT_ENABLED_BY_LACT.to_owned());
            }
            let _control = lock(&self.inner.control);
            self.refuse_disable_while_pending()?;
            lact.overdrive_dependencies()?
        } else {
            (Vec::new(), None)
        };

        let mut state = lock(&overdrive.state);
        Overdrive::check_idle(&state, self.inner.clock.now_ms())?;
        let expires_ms = self.inner.clock.now_ms().saturating_add(TOKEN_TTL_MS);
        let token = dependencies.is_empty().then(new_token);
        state.ticket = token.clone().map(|token| Ticket {
            action,
            token,
            expires_ms,
        });
        Ok(OverdrivePlan {
            action,
            steps,
            reboot_required: true,
            dependencies,
            saved_profiles,
            token,
            expires_ms,
        })
    }

    /// The last check before a disable leaves for lactd. Holds the control lock from the read
    /// until `disabling` is set, so no setter of Redline's lands in between.
    fn clear_for_disable(&self) -> Result<(), String> {
        let _control = lock(&self.inner.control);
        self.refuse_disable_while_pending()?;
        let (dependencies, _) = self.inner.lact.overdrive_dependencies()?;
        if !dependencies.is_empty() {
            return Err(DEPENDS_ON_IT.to_owned());
        }
        lock(&self.inner.overdrive.state).disabling = true;
        Ok(())
    }

    /// Sends `action` to lactd if `token` is the latest plan's, for the same action and not
    /// expired, and for a disable if nothing depends on overdrive right now. The token is spent
    /// by any call, right or wrong. Blocking, for minutes when lactd regenerates the initramfs.
    pub fn change_overdrive(
        &self,
        action: OverdriveAction,
        token: &str,
    ) -> Result<OverdriveResult, String> {
        let overdrive = &self.inner.overdrive;
        // An earlier disable left the outcome open: no later answer can settle it.
        let already_disabling;
        {
            let mut state = lock(&overdrive.state);
            Overdrive::check_idle(&state, self.inner.clock.now_ms())?;
            already_disabling = state.disabling;
            let ticket = state
                .ticket
                .take()
                .ok_or_else(|| NOT_CONFIRMED.to_owned())?;
            if ticket.action != action || ticket.token != token {
                return Err(NOT_CONFIRMED.to_owned());
            }
            if self.inner.clock.now_ms() >= ticket.expires_ms {
                return Err(CONFIRMATION_EXPIRED.to_owned());
            }
            state.busy = true;
        }
        if action == OverdriveAction::Disable
            && let Err(refusal) = self.clear_for_disable()
        {
            lock(&overdrive.state).busy = false;
            return Err(refusal);
        }

        let lact = &self.inner.lact;
        let sent = match action {
            OverdriveAction::Enable => lact.client.enable_overdrive(),
            OverdriveAction::Disable => lact.client.disable_overdrive(),
        };
        let mut state = lock(&overdrive.state);
        state.busy = false;
        if matches!(&sent, Err(LactError::Io(io)) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
        {
            // lactd keeps working after the connection is gone: hold every request back.
            state.stalled_until_ms = Some(
                self.inner
                    .clock
                    .now_ms()
                    .saturating_add(STALLED_COOLDOWN_MS),
            );
        } else if action == OverdriveAction::Disable
            && !already_disabling
            && sent.as_ref().is_err_and(proves_nothing_changed)
        {
            state.disabling = false;
        }
        match sent {
            Ok(()) => {
                state.reboot_pending = true;
                Ok(OverdriveResult {
                    reboot_required: true,
                })
            }
            Err(error) => {
                let (sentence, reboot_pending) = refusal(lact, &error);
                state.reboot_pending |= reboot_pending;
                Err(sentence)
            }
        }
    }
}

/// What enabling overdrive will do, with the token `enable_overdrive` needs. Changes nothing.
#[tauri::command]
pub async fn prepare_enable_overdrive(tuning: State<'_, Tuning>) -> Result<OverdrivePlan, String> {
    blocking(&tuning, |tuning| {
        tuning.prepare_overdrive(OverdriveAction::Enable)
    })
    .await?
}

/// Asks lactd to enable overdrive at the next boot. Only with the token of the plan the user
/// confirmed.
#[tauri::command]
pub async fn enable_overdrive(
    token: String,
    tuning: State<'_, Tuning>,
) -> Result<OverdriveResult, String> {
    blocking(&tuning, move |tuning| {
        tuning.change_overdrive(OverdriveAction::Enable, &token)
    })
    .await?
}

/// What disabling overdrive will do, with the token `disable_overdrive` needs. Changes nothing.
#[tauri::command]
pub async fn prepare_disable_overdrive(tuning: State<'_, Tuning>) -> Result<OverdrivePlan, String> {
    blocking(&tuning, |tuning| {
        tuning.prepare_overdrive(OverdriveAction::Disable)
    })
    .await?
}

/// Asks lactd to disable overdrive at the next boot. Only with the token of the plan the user
/// confirmed.
#[tauri::command]
pub async fn disable_overdrive(
    token: String,
    tuning: State<'_, Tuning>,
) -> Result<OverdriveResult, String> {
    blocking(&tuning, move |tuning| {
        tuning.change_overdrive(OverdriveAction::Disable, &token)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::super::fan::{FanSetting, FanSettings, PmfwSettings};
    use super::super::tests::{Collected, FakeLactd, Handler, ManualClock, Setup, TIMEOUT};
    use super::*;
    use crate::tuning::{NOT_MANAGED, Outcome, PendingChange, TuningChange};
    use redline_core::lact::{DeviceType, LactClient, VERIFIED_VERSION};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    const GPU_A: &str = "1002:164e-1043:8877-0000:0c:00.0";
    const GPU_B: &str = "1002:7550-1043:0643-0000:0d:00.0";
    const SLOT_A: &str = "0000:0c:00.0";
    const SLOT_B: &str = "0000:0d:00.0";

    /// A plan's token, for a plan that must have one.
    fn token(plan: &OverdrivePlan) -> &str {
        plan.token.as_deref().unwrap()
    }

    /// One device of the fake: what `list_devices` and `device_info` say about it.
    struct Device {
        id: &'static str,
        name: &'static str,
        driver: &'static str,
        pmfw: bool,
    }

    const FIRMWARE_FAN: Device = Device {
        id: GPU_B,
        name: "Discrete",
        driver: "amdgpu",
        pmfw: true,
    };
    const OLDER_FAN: Device = Device {
        id: GPU_A,
        name: "Integrated",
        driver: "amdgpu",
        pmfw: false,
    };

    fn curve() -> Value {
        json!({
            "fan_control_enabled": true,
            "fan_control_settings": { "mode": "curve", "curve": { "40": 0.3, "80": 1.0 } }
        })
    }

    /// lactd 0.10.1's overdrive rules (`lact-daemon/src/system.rs`): one enable or disable per
    /// daemon start, the bit of the running kernel unchanged until a reboot.
    struct Daemon {
        /// The running kernel's overdrive bit.
        enabled: bool,
        configurator: Value,
        /// `OC_TOGGLED`.
        toggled: bool,
        /// LACT wrote the modprobe file.
        file: bool,
        /// Every overdrive request received.
        log: Vec<String>,
        /// A refusal of the next overdrive request, as lactd words it.
        fail: Option<&'static str>,
        /// How long the next overdrive request takes.
        delay: Duration,
        devices: Vec<Device>,
        /// What `get_gpu_config` answers, by LACT id: the active profile's copy.
        configs: BTreeMap<String, Value>,
        /// Names of the stored profiles.
        profiles: Vec<&'static str>,
        active: Option<&'static str>,
        auto_switch: bool,
        /// `get_gpu_config` fails.
        unreadable: bool,
        /// lactd's one pending change: the configuration a revert puts back.
        pending: Option<(String, Option<Value>)>,
        /// Every setter received, with its arguments.
        setters: Vec<Value>,
    }

    impl Daemon {
        /// `edit_gpu_config`: applies at once, holds the previous configuration, 5 s to confirm.
        fn edit(&mut self, id: &str, edit: impl FnOnce(&mut Value)) -> Value {
            if self.pending.is_some() {
                return json!({ "error": "There is an unconfirmed configuration change pending" });
            }
            let previous = self.configs.get(id).cloned();
            let mut config = previous.clone().unwrap_or_else(|| json!({}));
            edit(&mut config);
            self.configs.insert(id.to_owned(), config);
            self.pending = Some((id.to_owned(), previous));
            json!(5)
        }
    }

    fn lactd(daemon: Arc<Mutex<Daemon>>) -> Box<Handler> {
        Box::new(move |command, args| {
            let mut daemon = daemon.lock().unwrap();
            match command {
                "system_info" => json!({
                    "version": VERIFIED_VERSION, "distro": "Ubuntu",
                    "amdgpu_overdrive_enabled": daemon.enabled,
                    "amdgpu_params_configurator": daemon.configurator
                }),
                "list_devices" => daemon
                    .devices
                    .iter()
                    .map(|device| json!({ "id": device.id, "name": device.name }))
                    .collect(),
                "device_info" => {
                    let device = daemon
                        .devices
                        .iter()
                        .find(|device| device.id == args["id"])
                        .unwrap();
                    let flags = if device.pmfw { vec!["HasPmfw"] } else { vec![] };
                    json!({ "driver": device.driver, "flags": flags })
                }
                "get_gpu_config" if daemon.unreadable => json!({ "error": "no config" }),
                "get_gpu_config" => daemon
                    .configs
                    .get(args["id"].as_str().unwrap())
                    .cloned()
                    .into(),
                "list_profiles" => {
                    let names: serde_json::Map<String, Value> = daemon
                        .profiles
                        .iter()
                        .map(|name| ((*name).to_owned(), Value::Null))
                        .collect();
                    json!({
                        "profiles": names, "current_profile": daemon.active,
                        "auto_switch": daemon.auto_switch
                    })
                }
                "set_fan_control" => {
                    daemon.setters.push(args.clone());
                    let (enabled, pmfw) = (args["enabled"].clone(), args["pmfw"].clone());
                    daemon.edit(args["id"].as_str().unwrap(), |config| {
                        config["fan_control_enabled"] = enabled;
                        config["pmfw_options"] = pmfw;
                    })
                }
                "set_clocks_value" => {
                    daemon.setters.push(args.clone());
                    daemon.edit(args["id"].as_str().unwrap(), |config| {
                        config.as_object_mut().unwrap().remove("voltage_offset");
                    })
                }
                "confirm_pending_config" => {
                    let (id, previous) = daemon.pending.take().unwrap();
                    if args["command"] == "revert" {
                        match previous {
                            Some(previous) => daemon.configs.insert(id, previous),
                            None => daemon.configs.remove(&id),
                        };
                    }
                    Value::Null
                }
                "enable_overdrive" | "disable_overdrive" => {
                    daemon.log.push(command.to_owned());
                    std::thread::sleep(std::mem::take(&mut daemon.delay));
                    if let Some(message) = daemon.fail.take() {
                        return json!({ "error": message });
                    }
                    if daemon.toggled {
                        return json!({ "error": "Overdrive support was already toggled - please reboot to apply the changes" });
                    }
                    if command == "enable_overdrive" {
                        if daemon.enabled {
                            return json!({ "error": "Overdrive mask already enabled" });
                        }
                        daemon.file = true;
                    } else if !daemon.file {
                        return json!({ "error": "Overclocking was not enabled through LACT (file at /etc/modprobe.d/99-amdgpu-overdrive.conf does not exist)" });
                    } else {
                        daemon.file = false;
                    }
                    daemon.toggled = true;
                    json!("")
                }
                other => json!({ "error": format!("unexpected {other}") }),
            }
        })
    }

    struct Service {
        _setup: Setup,
        daemon: Arc<Mutex<Daemon>>,
        clock: Arc<ManualClock>,
        tuning: Tuning,
        _lactd: FakeLactd,
    }

    impl Service {
        fn start(name: &str, overdrive_timeout: Duration) -> Self {
            let setup = Setup::new(&format!("overdrive-{name}"));
            let daemon = Arc::new(Mutex::new(Daemon {
                enabled: false,
                configurator: json!({ "Modprobe": "Debian" }),
                toggled: false,
                file: false,
                log: Vec::new(),
                fail: None,
                delay: Duration::ZERO,
                devices: Vec::new(),
                configs: BTreeMap::new(),
                profiles: Vec::new(),
                active: None,
                auto_switch: false,
                unreadable: false,
                pending: None,
                setters: Vec::new(),
            }));
            let lactd = FakeLactd::start(&setup.socket(), lactd(daemon.clone()));
            let clock = Arc::new(ManualClock::default());
            let modprobe_file = setup.socket().with_file_name("overdrive.conf");
            std::fs::write(&modprobe_file, "").unwrap();
            let lact = Lact::new(
                LactClient::new(setup.socket())
                    .with_timeout(TIMEOUT)
                    .with_overdrive_timeout(overdrive_timeout),
                std::ffi::OsString::new(),
            )
            .with_modprobe_file(modprobe_file);
            let tuning = Tuning::new(lact, clock.clone(), Arc::new(Collected::default()));
            Service {
                _setup: setup,
                daemon,
                clock,
                tuning,
                _lactd: lactd,
            }
        }

        fn new(name: &str) -> Self {
            Service::start(name, TIMEOUT)
        }

        fn log(&self) -> Vec<String> {
            std::mem::take(&mut self.daemon.lock().unwrap().log)
        }

        /// lactd with overdrive on, enabled through LACT, and these devices.
        fn on(name: &str, devices: Vec<Device>) -> Self {
            let service = Service::new(name);
            {
                let mut daemon = service.daemon.lock().unwrap();
                daemon.enabled = true;
                daemon.file = true;
                daemon.devices = devices;
            }
            service
        }

        fn store(&self, id: &str, config: Value) {
            let configs = &mut self.daemon.lock().unwrap().configs;
            configs.insert(id.to_owned(), config);
        }

        fn sentences(&self) -> Vec<String> {
            let plan = self.tuning.prepare_overdrive(DISABLE).unwrap();
            plan.dependencies.into_iter().map(|d| d.sentence).collect()
        }
    }

    const ENABLE: OverdriveAction = OverdriveAction::Enable;
    const DISABLE: OverdriveAction = OverdriveAction::Disable;

    #[test]
    fn a_confirmed_plan_enables_overdrive_for_the_next_boot() {
        let service = Service::new("enable");
        service.clock.advance(1_000);
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert_eq!(plan.action, ENABLE);
        assert!(plan.reboot_required);
        assert_eq!(plan.expires_ms, 1_000 + TOKEN_TTL_MS);
        assert_eq!(token(&plan).len(), 32);
        assert_eq!(
            plan.steps,
            vec![
                format!(
                    "LACT writes {OVERDRIVE_MODPROBE_FILE}, which adds the overdrive bit to the \
                     `amdgpu.ppfeaturemask` kernel module option."
                ),
                "LACT regenerates the initramfs with `update-initramfs -u`; this can take a few \
                 minutes."
                    .to_owned(),
                "The change takes effect after a reboot. Redline does not reboot the machine."
                    .to_owned(),
            ]
        );
        // Preparing changes nothing.
        assert_eq!(service.log(), Vec::<String>::new());

        assert_eq!(
            service.tuning.change_overdrive(ENABLE, token(&plan)),
            Ok(OverdriveResult {
                reboot_required: true
            })
        );
        assert_eq!(service.log(), vec!["enable_overdrive"]);

        // The running kernel keeps its value until the reboot; the session knows one is due.
        let status = service.tuning.status("0000:0d:00.0");
        assert_eq!(status.overdrive_enabled, Some(false));
        assert!(status.reboot_pending);
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["reboot_pending"], true);

        // Nothing more until the reboot: no new plan, and the spent token is worth nothing.
        assert_eq!(
            service.tuning.prepare_overdrive(DISABLE),
            Err(REBOOT_PENDING.to_owned())
        );
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, token(&plan)),
            Err(REBOOT_PENDING.to_owned())
        );
        assert_eq!(service.log(), Vec::<String>::new());
    }

    #[test]
    fn without_the_token_of_a_plan_nothing_reaches_lactd() {
        let service = Service::new("token");
        // No plan at all.
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, ""),
            Err(NOT_CONFIRMED.to_owned())
        );
        // A wrong token, which also spends the plan's.
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert_eq!(
            service
                .tuning
                .change_overdrive(ENABLE, "0123456789abcdef0123456789abcdef"),
            Err(NOT_CONFIRMED.to_owned())
        );
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, token(&plan)),
            Err(NOT_CONFIRMED.to_owned())
        );
        // A plan to disable does not enable.
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(plan.action, DISABLE);
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, token(&plan)),
            Err(NOT_CONFIRMED.to_owned())
        );
        // A newer plan replaces the older one's token.
        let older = service.tuning.prepare_overdrive(ENABLE).unwrap();
        let newer = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert_ne!(older.token, newer.token);
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, token(&older)),
            Err(NOT_CONFIRMED.to_owned())
        );
        assert_eq!(service.log(), Vec::<String>::new());
        assert!(!service.tuning.status("0000:0d:00.0").reboot_pending);
    }

    #[test]
    fn an_expired_confirmation_is_refused() {
        let service = Service::new("expired");
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        service.clock.advance(TOKEN_TTL_MS);
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, token(&plan)),
            Err(CONFIRMATION_EXPIRED.to_owned())
        );
        assert_eq!(service.log(), Vec::<String>::new());
        // A fresh plan works.
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert!(
            service
                .tuning
                .change_overdrive(ENABLE, token(&plan))
                .is_ok()
        );
    }

    #[test]
    fn a_lactd_refusal_is_a_sentence_and_no_reboot_is_pending() {
        let service = Service::new("refused");
        service.daemon.lock().unwrap().fail = Some("Initramfs generation command failed");
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, token(&plan)),
            Err("LACT could not change overdrive: Initramfs generation command failed".to_owned())
        );
        assert!(!service.tuning.status("0000:0d:00.0").reboot_pending);

        // Disabling what LACT did not enable.
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&plan)),
            Err(NOT_ENABLED_BY_LACT.to_owned())
        );
        assert_eq!(service.log(), vec!["enable_overdrive", "disable_overdrive"]);
    }

    #[test]
    fn lactd_already_toggled_means_a_reboot_is_pending() {
        // Changed in an earlier Redline session, or from the LACT app, since lactd started.
        let service = Service::new("toggled");
        service.daemon.lock().unwrap().toggled = true;
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, token(&plan)),
            Err(REBOOT_PENDING.to_owned())
        );
        assert!(service.tuning.status("0000:0d:00.0").reboot_pending);
    }

    #[test]
    fn enabling_what_is_already_on_is_refused_before_lactd() {
        let service = Service::new("on");
        service.daemon.lock().unwrap().enabled = true;
        assert_eq!(
            service.tuning.prepare_overdrive(ENABLE),
            Err(ALREADY_ENABLED.to_owned())
        );
        // Disabling it is offered the same way.
        service.daemon.lock().unwrap().file = true;
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(
            plan.steps[0],
            format!("LACT deletes {OVERDRIVE_MODPROBE_FILE}, the file it wrote to enable it.")
        );
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&plan)),
            Ok(OverdriveResult {
                reboot_required: true
            })
        );
        assert_eq!(service.log(), vec!["disable_overdrive"]);
        assert!(service.tuning.status("0000:0d:00.0").reboot_pending);
    }

    #[test]
    fn without_a_known_configurator_there_is_no_plan() {
        let service = Service::new("nixos");
        service.daemon.lock().unwrap().configurator = Value::Null;
        assert_eq!(
            service.tuning.prepare_overdrive(ENABLE),
            Err(NO_CONFIGURATOR.to_owned())
        );
        service.daemon.lock().unwrap().configurator = json!({ "BootArg": "Grubby" });
        let error = service.tuning.prepare_overdrive(ENABLE).unwrap_err();
        assert!(error.contains("Grubby"), "{error}");
        assert_eq!(service.log(), Vec::<String>::new());
    }

    #[test]
    fn the_steps_follow_the_configurator() {
        let rpm = steps(ENABLE, Some(ParamsConfigurator::RpmOstree)).unwrap();
        assert!(rpm[0].contains("rpm-ostree kargs"), "{rpm:?}");
        assert_eq!(rpm.len(), 2);
        let plain = steps(DISABLE, Some(ParamsConfigurator::Modprobe(None))).unwrap();
        assert_eq!(plain.len(), 2, "no initramfs to regenerate: {plain:?}");
        let dracut = steps(
            ENABLE,
            Some(ParamsConfigurator::Modprobe(Some(
                redline_core::lact::InitramfsType::Dracut,
            ))),
        )
        .unwrap();
        assert!(dracut[1].contains("dracut --regenerate-all --force"));
    }

    #[test]
    fn an_unreachable_lactd_is_its_usual_sentence() {
        let service = Service::new("down");
        drop(service._lactd);
        let error = service.tuning.prepare_overdrive(ENABLE).unwrap_err();
        assert!(error.contains("LACT"), "{error}");
    }

    #[test]
    fn a_slow_lactd_is_waited_for_and_refuses_a_second_request_meanwhile() {
        let service = Arc::new(Service::start("slow", TIMEOUT));
        service.daemon.lock().unwrap().delay = Duration::from_millis(300);
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        let running = {
            let service = service.clone();
            std::thread::spawn(move || service.tuning.change_overdrive(ENABLE, token(&plan)))
        };
        // The request has left: a plan sent before that would replace the token it carries.
        std::thread::sleep(Duration::from_millis(100));
        // While lactd works, no new plan.
        let mut busy = false;
        for _ in 0..100 {
            if service.tuning.prepare_overdrive(DISABLE) == Err(BUSY.to_owned()) {
                busy = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(busy);
        assert_eq!(
            running.join().unwrap(),
            Ok(OverdriveResult {
                reboot_required: true
            })
        );
    }

    #[test]
    fn a_lactd_that_does_not_answer_in_time_is_named() {
        let service = Service::start("timeout", Duration::from_millis(100));
        service.daemon.lock().unwrap().delay = Duration::from_millis(400);
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, token(&plan)),
            Err(TIMED_OUT.to_owned())
        );
        // Not known to be done: no reboot is claimed.
        assert!(!service.tuning.status("0000:0d:00.0").reboot_pending);
        std::thread::sleep(Duration::from_millis(400));
        // lactd may still be regenerating the initramfs: nothing more is sent for a while.
        assert_eq!(
            service.tuning.prepare_overdrive(ENABLE),
            Err(STILL_RUNNING.to_owned())
        );
        service.clock.advance(STALLED_COOLDOWN_MS - 1);
        assert_eq!(
            service.tuning.prepare_overdrive(DISABLE),
            Err(STILL_RUNNING.to_owned())
        );
        assert_eq!(service.log(), vec!["enable_overdrive"]);
        service.clock.advance(1);
        assert!(service.tuning.prepare_overdrive(ENABLE).is_ok());
    }

    #[test]
    fn the_list_reads_every_gpu_and_names_gpu_setting_and_profile() {
        let service = Service::on("list", vec![OLDER_FAN, FIRMWARE_FAN]);
        {
            let mut daemon = service.daemon.lock().unwrap();
            daemon.profiles = vec!["Gaming", "Quiet"];
            daemon.active = Some("Gaming");
        }
        service.store(GPU_A, json!({ "voltage_offset": -50 }));
        service.store(GPU_B, curve());
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(
            plan.dependencies,
            vec![
                OverdriveDependency {
                    gpu_id: SLOT_A.to_owned(),
                    kind: OverdriveDependencyKind::Clocks,
                    profile: Some("Gaming".to_owned()),
                    sentence: "Integrated has clock or voltage values from the active profile \
                               \"Gaming\"."
                        .to_owned(),
                },
                OverdriveDependency {
                    gpu_id: SLOT_B.to_owned(),
                    kind: OverdriveDependencyKind::Fan,
                    profile: Some("Gaming".to_owned()),
                    sentence: "Discrete has a fan curve from the active profile \"Gaming\"."
                        .to_owned(),
                },
            ]
        );
        // Saved profiles do not block, and the plan says so.
        assert_eq!(plan.saved_profiles.as_deref(), Some(SAVED_PROFILES));
        // No token at all: there is nothing to send.
        assert_eq!(plan.token, None);
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, ""),
            Err(NOT_CONFIRMED.to_owned())
        );
        assert_eq!(service.log(), Vec::<String>::new());
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(json["dependencies"][1]["kind"], "fan");
        assert_eq!(json["token"], Value::Null);
    }

    #[test]
    fn a_blocked_plan_cancels_the_token_of_the_plan_before_it() {
        let service = Service::on("older", vec![FIRMWARE_FAN]);
        let clear = service.tuning.prepare_overdrive(DISABLE).unwrap();
        service.store(GPU_B, curve());
        assert_eq!(
            service.tuning.prepare_overdrive(DISABLE).unwrap().token,
            None
        );
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&clear)),
            Err(NOT_CONFIRMED.to_owned())
        );
    }

    #[test]
    fn each_firmware_fan_setting_has_its_own_words_outside_a_profile() {
        let service = Service::on("words", vec![FIRMWARE_FAN]);
        let fixed = json!({
            "fan_control_enabled": true, "fan_control_settings": { "mode": "static" }
        });
        service.store(GPU_B, fixed);
        assert_eq!(
            service.sentences(),
            ["Discrete has a fixed fan speed stored in LACT."]
        );
        service.store(GPU_B, json!({ "pmfw_options": { "zero_rpm": false } }));
        assert_eq!(
            service.sentences(),
            ["Discrete has firmware fan options stored in LACT."]
        );
        let mut both = curve();
        both["pmfw_options"] = json!({ "acoustic_limit": 2000 });
        service.store(GPU_B, both);
        assert_eq!(
            service.sentences(),
            ["Discrete has a fan curve and firmware fan options stored in LACT."]
        );
    }

    #[test]
    fn what_does_not_need_overdrive_does_not_block() {
        let other = Device {
            id: "10de:2704-1462:5110-0000:09:00.0",
            name: "Other vendor",
            driver: "nvidia",
            pmfw: false,
        };
        let service = Service::on("free", vec![OLDER_FAN, FIRMWARE_FAN, other]);
        // A fan LACT drives itself, a curve left stored but not in use, another driver's clocks.
        service.store(GPU_A, curve());
        let mut unused = curve();
        unused["fan_control_enabled"] = json!(false);
        unused["power_cap"] = json!(200.0);
        service.store(GPU_B, unused);
        service.store(
            "10de:2704-1462:5110-0000:09:00.0",
            json!({ "max_core_clock": 3000 }),
        );
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(plan.dependencies, vec![]);
        assert_eq!(plan.saved_profiles, None);
        assert!(
            service
                .tuning
                .change_overdrive(DISABLE, token(&plan))
                .is_ok()
        );
        assert_eq!(service.log(), vec!["disable_overdrive"]);
    }

    #[test]
    fn a_list_that_changed_after_the_plan_refuses_the_disable() {
        let service = Service::on("changed", vec![FIRMWARE_FAN]);
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        service.store(GPU_B, curve());
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&plan)),
            Err(DEPENDS_ON_IT.to_owned())
        );
        assert_eq!(service.log(), Vec::<String>::new());
        // Refused, not stuck: the token is spent and the next plan shows the list.
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&plan)),
            Err(NOT_CONFIRMED.to_owned())
        );
        assert_eq!(service.sentences().len(), 1);
        assert!(!service.tuning.status(SLOT_B).reboot_pending);
    }

    #[test]
    fn a_list_that_cannot_be_read_never_counts_as_empty() {
        let service = Service::on("unreadable", vec![FIRMWARE_FAN]);
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        service.daemon.lock().unwrap().unreadable = true;
        let refused = service.tuning.change_overdrive(DISABLE, token(&plan));
        assert!(refused.unwrap_err().contains("cannot tell"));
        let refused = service.tuning.prepare_overdrive(DISABLE);
        assert!(refused.unwrap_err().contains("cannot tell"));
        assert_eq!(service.log(), Vec::<String>::new());
    }

    #[test]
    fn a_confirmed_reset_of_the_fan_clears_the_list_and_lets_the_disable_through() {
        let service = Service::on("confirmed", vec![FIRMWARE_FAN]);
        let mut both = curve();
        both["pmfw_options"] = json!({ "acoustic_limit": 2000 });
        service.store(GPU_B, both);
        assert_eq!(service.sentences().len(), 1);

        let change = service.tuning.reset_fan(SLOT_B).unwrap();
        assert_eq!(change.what, TuningChange::Fan);
        // Nothing was confirmed for the user, and the mode stays as stored.
        let sent = service.daemon.lock().unwrap().setters.remove(0);
        assert_eq!(sent["enabled"], false);
        assert_eq!(sent["mode"], Value::Null);
        assert_eq!(sent["pmfw"]["acoustic_limit"], Value::Null);

        // While it is pending a disable is refused: a revert would bring the curve back.
        assert_eq!(
            service.tuning.prepare_overdrive(DISABLE),
            Err(PENDING_FIRST.to_owned())
        );
        assert_eq!(service.tuning.settle(SLOT_B, true), Ok(Outcome::Confirmed));
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(plan.dependencies, vec![]);
        assert!(
            service
                .tuning
                .change_overdrive(DISABLE, token(&plan))
                .is_ok()
        );
        assert_eq!(service.log(), vec!["disable_overdrive"]);
    }

    #[test]
    fn a_reverted_reset_keeps_the_item_listed() {
        let service = Service::on("reverted", vec![OLDER_FAN]);
        service.store(GPU_A, json!({ "voltage_offset": -50 }));
        service.tuning.reset_clocks(SLOT_A).unwrap();
        assert_eq!(service.tuning.settle(SLOT_A, false), Ok(Outcome::Reverted));
        assert_eq!(service.sentences().len(), 1);
    }

    #[test]
    fn a_change_pending_when_the_disable_is_sent_stops_it() {
        let service = Service::on("pending", vec![FIRMWARE_FAN]);
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        let change: PendingChange = service.tuning.reset_fan(SLOT_B).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&plan)),
            Err(PENDING_FIRST.to_owned())
        );
        assert_eq!(service.log(), Vec::<String>::new());
        assert_eq!(service.tuning.pending(SLOT_B), Some(change));
    }

    #[test]
    fn once_disabling_nothing_that_needs_overdrive_can_be_set() {
        let service = Service::on("after", vec![FIRMWARE_FAN]);
        {
            let mut daemon = service.daemon.lock().unwrap();
            daemon.profiles = vec!["Gaming"];
        }
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        service
            .tuning
            .change_overdrive(DISABLE, token(&plan))
            .unwrap();

        // The running kernel still has overdrive: lactd would accept these for the next boot.
        let curve = FanSettings {
            mode: FanSetting::Curve,
            static_speed: None,
            curve: Some(Vec::new()),
            pmfw: PmfwSettings::default(),
        };
        assert_eq!(
            service.tuning.set_fan(SLOT_B, &curve),
            Err(DISABLE_UNDER_WAY.to_owned())
        );
        let values = serde_json::Map::from_iter([("voltage_offset".to_owned(), json!(-50))]);
        assert_eq!(
            service.tuning.set_clocks(SLOT_B, &values),
            Err(DISABLE_UNDER_WAY.to_owned())
        );
        assert_eq!(
            service.tuning.activate_profile(SLOT_B, Some("Gaming")),
            Err(DISABLE_UNDER_WAY.to_owned())
        );
        // The ways back stay open.
        assert!(service.tuning.reset_fan(SLOT_B).is_ok());
        assert_eq!(service.tuning.settle(SLOT_B, true), Ok(Outcome::Confirmed));
        assert!(service.tuning.reset_clocks(SLOT_B).is_ok());
    }

    #[test]
    fn a_refused_disable_lifts_the_stop_and_a_timed_out_one_keeps_it() {
        let service = Service::start("refused-stop", Duration::from_millis(100));
        service.daemon.lock().unwrap().enabled = true;
        // lactd refuses: nothing was disabled.
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&plan)),
            Err(NOT_ENABLED_BY_LACT.to_owned())
        );
        assert_eq!(
            service.tuning.inner.overdrive.refuse_while_disabling(),
            Ok(())
        );

        // lactd does not answer in time: it may still finish.
        service.daemon.lock().unwrap().file = true;
        service.daemon.lock().unwrap().delay = Duration::from_millis(400);
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&plan)),
            Err(TIMED_OUT.to_owned())
        );
        assert_eq!(
            service.tuning.inner.overdrive.refuse_while_disabling(),
            Err(DISABLE_UNDER_WAY.to_owned())
        );
    }

    #[test]
    fn a_disable_that_may_have_changed_something_keeps_the_stop() {
        let stopped = Err(DISABLE_UNDER_WAY.to_owned());

        // Changed since lactd started, by this request's predecessor or by another program.
        let service = Service::on("toggled-stop", vec![]);
        service.daemon.lock().unwrap().toggled = true;
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&plan)),
            Err(REBOOT_PENDING.to_owned())
        );
        assert!(service.tuning.status(SLOT_B).reboot_pending);
        assert_eq!(
            service.tuning.inner.overdrive.refuse_while_disabling(),
            stopped
        );

        // lactd removed its file before the initramfs failed. A retry, told the file is gone,
        // proves nothing about the first attempt.
        let service = Service::on("initramfs-stop", vec![]);
        service.daemon.lock().unwrap().fail = Some("Initramfs generation command failed");
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert!(
            service
                .tuning
                .change_overdrive(DISABLE, token(&plan))
                .is_err()
        );
        assert_eq!(
            service.tuning.inner.overdrive.refuse_while_disabling(),
            stopped
        );
        service.daemon.lock().unwrap().file = false;
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, token(&plan)),
            Err(NOT_ENABLED_BY_LACT.to_owned())
        );
        assert_eq!(
            service.tuning.inner.overdrive.refuse_while_disabling(),
            stopped
        );

        // A failed enable says nothing about an earlier disable.
        service.daemon.lock().unwrap().enabled = false;
        service.daemon.lock().unwrap().fail = Some("Initramfs generation command failed");
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert!(
            service
                .tuning
                .change_overdrive(ENABLE, token(&plan))
                .is_err()
        );
        assert_eq!(
            service.tuning.inner.overdrive.refuse_while_disabling(),
            stopped
        );
    }

    #[test]
    fn only_a_request_that_did_nothing_proves_nothing_changed() {
        let daemon = |message: &str| LactError::Daemon(message.to_owned());
        assert!(proves_nothing_changed(&LactError::SocketMissing));
        assert!(proves_nothing_changed(&LactError::NotRunning));
        assert!(proves_nothing_changed(&LactError::PermissionDenied));
        assert!(proves_nothing_changed(&daemon(
            "Overclocking was not enabled through LACT (file at x does not exist)"
        )));
        assert!(proves_nothing_changed(&daemon(
            "Overdrive should be toggled through system config on NixOS"
        )));
        assert!(!proves_nothing_changed(&daemon(
            "Overdrive support was already toggled - please reboot to apply the changes"
        )));
        assert!(!proves_nothing_changed(&daemon("Initramfs failed")));
        assert!(!proves_nothing_changed(&LactError::Protocol(
            "?".to_owned()
        )));
        assert!(!proves_nothing_changed(&LactError::Io(
            std::io::ErrorKind::ConnectionReset.into()
        )));
    }

    #[test]
    fn a_missing_lact_file_is_said_before_any_setting_is_put_back() {
        let service = Service::on("no-file", vec![FIRMWARE_FAN]);
        service.store(GPU_B, curve());
        let file = service._setup.socket().with_file_name("overdrive.conf");
        std::fs::remove_file(file).unwrap();
        assert_eq!(
            service.tuning.prepare_overdrive(DISABLE),
            Err(NOT_ENABLED_BY_LACT.to_owned())
        );
        // Enabling never looks at it.
        service.daemon.lock().unwrap().enabled = false;
        assert!(service.tuning.prepare_overdrive(ENABLE).is_ok());
    }

    #[test]
    fn automatic_switching_is_part_of_the_note_on_saved_profiles() {
        let service = Service::on("auto", vec![]);
        {
            let mut daemon = service.daemon.lock().unwrap();
            daemon.profiles = vec!["Gaming", "Quiet"];
            daemon.active = Some("Gaming");
            daemon.auto_switch = true;
        }
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        let note = plan.saved_profiles.unwrap();
        assert!(note.starts_with(SAVED_PROFILES) && note.ends_with(AUTO_SWITCH));
        // Only one profile, and it is the active one: nothing to say.
        service.daemon.lock().unwrap().profiles = vec!["Gaming"];
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(plan.saved_profiles, None);
    }

    #[test]
    fn a_device_with_no_name_is_named_by_its_slot() {
        let device = DeviceListEntry {
            id: GPU_B.to_owned(),
            name: None,
            device_type: DeviceType::default(),
        };
        let config = GpuConfig {
            fan_control_enabled: true,
            ..GpuConfig::default()
        };
        let items = dependencies(&device, true, &config, None);
        assert_eq!(
            items[0].sentence,
            format!("{SLOT_B} has a fan setting stored in LACT.")
        );
    }

    #[test]
    fn an_unknown_gpu_is_refused_by_the_reset_too() {
        let service = Service::on("unknown", vec![]);
        assert_eq!(
            service.tuning.reset_fan(SLOT_B),
            Err(NOT_MANAGED.to_owned())
        );
    }
}
