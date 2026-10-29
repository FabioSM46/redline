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

use super::{Lact, Tuning, blocking, lock};
use redline_core::lact::{LactError, OVERDRIVE_MODPROBE_FILE, ParamsConfigurator};
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
    /// Pass it to `enable_overdrive` / `disable_overdrive` once the user confirms. Single use.
    pub token: String,
    /// When the token stops being accepted, in milliseconds since the Unix epoch.
    pub expires_ms: u64,
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

impl Tuning {
    /// Reads how lactd would do `action` and returns the plan to confirm. A new plan replaces
    /// the previous one's token. Blocking.
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
        let steps = steps(action, system.params_configurator())?;

        let mut state = lock(&overdrive.state);
        Overdrive::check_idle(&state, self.inner.clock.now_ms())?;
        let ticket = Ticket {
            action,
            token: new_token(),
            expires_ms: self.inner.clock.now_ms().saturating_add(TOKEN_TTL_MS),
        };
        let plan = OverdrivePlan {
            action,
            steps,
            reboot_required: true,
            token: ticket.token.clone(),
            expires_ms: ticket.expires_ms,
        };
        state.ticket = Some(ticket);
        Ok(plan)
    }

    /// Sends `action` to lactd if `token` is the latest plan's, for the same action and not
    /// expired. The token is spent by any call, right or wrong. Blocking, for minutes when
    /// lactd regenerates the initramfs.
    pub fn change_overdrive(
        &self,
        action: OverdriveAction,
        token: &str,
    ) -> Result<OverdriveResult, String> {
        let overdrive = &self.inner.overdrive;
        {
            let mut state = lock(&overdrive.state);
            Overdrive::check_idle(&state, self.inner.clock.now_ms())?;
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
    use super::super::tests::{Collected, FakeLactd, Handler, ManualClock, Setup, TIMEOUT};
    use super::*;
    use redline_core::lact::{LactClient, VERIFIED_VERSION};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::time::Duration;

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
    }

    fn lactd(daemon: Arc<Mutex<Daemon>>) -> Box<Handler> {
        Box::new(move |command, _args| {
            let mut daemon = daemon.lock().unwrap();
            match command {
                "system_info" => json!({
                    "version": VERIFIED_VERSION, "distro": "Ubuntu",
                    "amdgpu_overdrive_enabled": daemon.enabled,
                    "amdgpu_params_configurator": daemon.configurator
                }),
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
            }));
            let lactd = FakeLactd::start(&setup.socket(), lactd(daemon.clone()));
            let clock = Arc::new(ManualClock::default());
            let lact = Lact::new(
                LactClient::new(setup.socket())
                    .with_timeout(TIMEOUT)
                    .with_overdrive_timeout(overdrive_timeout),
                std::ffi::OsString::new(),
            );
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
        assert_eq!(plan.token.len(), 32);
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
            service.tuning.change_overdrive(ENABLE, &plan.token),
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
            service.tuning.change_overdrive(ENABLE, &plan.token),
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
            service.tuning.change_overdrive(ENABLE, &plan.token),
            Err(NOT_CONFIRMED.to_owned())
        );
        // A plan to disable does not enable.
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(plan.action, DISABLE);
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, &plan.token),
            Err(NOT_CONFIRMED.to_owned())
        );
        // A newer plan replaces the older one's token.
        let older = service.tuning.prepare_overdrive(ENABLE).unwrap();
        let newer = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert_ne!(older.token, newer.token);
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, &older.token),
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
            service.tuning.change_overdrive(ENABLE, &plan.token),
            Err(CONFIRMATION_EXPIRED.to_owned())
        );
        assert_eq!(service.log(), Vec::<String>::new());
        // A fresh plan works.
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert!(service.tuning.change_overdrive(ENABLE, &plan.token).is_ok());
    }

    #[test]
    fn a_lactd_refusal_is_a_sentence_and_no_reboot_is_pending() {
        let service = Service::new("refused");
        service.daemon.lock().unwrap().fail = Some("Initramfs generation command failed");
        let plan = service.tuning.prepare_overdrive(ENABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(ENABLE, &plan.token),
            Err("LACT could not change overdrive: Initramfs generation command failed".to_owned())
        );
        assert!(!service.tuning.status("0000:0d:00.0").reboot_pending);

        // Disabling what LACT did not enable.
        let plan = service.tuning.prepare_overdrive(DISABLE).unwrap();
        assert_eq!(
            service.tuning.change_overdrive(DISABLE, &plan.token),
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
            service.tuning.change_overdrive(ENABLE, &plan.token),
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
            service.tuning.change_overdrive(DISABLE, &plan.token),
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
            std::thread::spawn(move || service.tuning.change_overdrive(ENABLE, &plan.token))
        };
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
            service.tuning.change_overdrive(ENABLE, &plan.token),
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
}
