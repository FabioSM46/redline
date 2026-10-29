//! LACT profiles: list, create, activate (with the user's confirmation) and delete.
//!
//! Verified against lactd 0.10.1 (`list_profiles`, `set_profile`, `create_profile`,
//! `delete_profile` and `set_current_profile` in `lact-daemon/src/server/handler.rs`):
//!
//! - `set_profile` is **not** a pending change. It applies the profile's configuration to every
//!   GPU at once, saves it, and answers with no timer: `confirm_pending_config` has nothing to
//!   confirm and lactd never reverts it. AGENTS.md wants every tuning change confirmed, so Redline
//!   holds the switch as its own [`PendingChange`] (`what: "profile"`) for [`CONFIRM_SECONDS`]:
//!   `tuning_confirm` keeps it without a request, `tuning_revert`, the deadline or quitting the
//!   app send `set_profile` with the previous profile. Nothing confirms on its own.
//! - `set_profile` does not check lactd's pending change, and lactd's revert of a pending
//!   setter writes into whatever profile is active by then. So a switch is refused while any
//!   change is pending, and every other change is refused while a switch is pending.
//! - `create_profile` and `delete_profile` save lactd's whole configuration to disk, including a
//!   change still waiting for confirmation: both are refused while one is pending.
//! - Deleting the **active** profile makes lactd switch to the default one at once, without a
//!   confirmation: Redline refuses it and asks to switch first.
//! - `auto_switch` is always sent `false`: Redline never turns LACT's automatic switching on, and
//!   while LACT has it on, a manual switch (which would turn it off for good) is refused.

use super::{PendingChange, Tuning, TuningChange, blocking, lock};
use redline_core::lact::{LactError, ProfileBase, ProfilesInfo};
use serde::Serialize;
use tauri::State;

/// Emitted with the new [`Profiles`] after a profile is created, activated, restored or deleted.
pub const PROFILE_EVENT: &str = "tuning-profile";
/// How long a profile switch waits for the user's answer before Redline switches back.
pub const CONFIRM_SECONDS: u64 = 15;
/// The longest profile name Redline accepts, in characters.
pub const MAX_NAME_CHARS: usize = 64;

pub const AUTO_SWITCH_ON: &str = "LACT is switching profiles automatically. Turn automatic \
    switching off in LACT to choose a profile here.";
pub const ALREADY_ACTIVE: &str = "This profile is already active.";
pub const ACTIVE_PROFILE: &str = "This profile is active: switch to another profile before \
    deleting it.";
pub const PENDING_CHANGE: &str = "A change is still waiting for confirmation: confirm or revert \
    it before changing profiles.";
pub const PROFILE_HELD: &str = "Another program is holding a LACT profile: try again when it \
    releases it.";
pub const EMPTY_NAME: &str = "A profile needs a name.";
pub const CONTROL_CHARACTERS: &str = "A profile name cannot contain line breaks or other \
    control characters.";
pub const RESERVED_NAME: &str = "\"Default\" is the name of LACT's default profile: choose \
    another name.";

/// The profiles LACT stores: the payload of `list_profiles` and of [`PROFILE_EVENT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profiles {
    /// The stored profiles, sorted by name.
    pub profiles: Vec<String>,
    /// The active profile; `null` is LACT's default profile.
    pub active: Option<String>,
    /// LACT switches profiles by itself (set up in LACT): manual switching is refused.
    pub auto_switch: bool,
}

impl From<ProfilesInfo> for Profiles {
    fn from(info: ProfilesInfo) -> Self {
        Profiles {
            profiles: info.profiles,
            active: info.current_profile,
            auto_switch: info.auto_switch,
        }
    }
}

fn no_such_profile(name: &str) -> String {
    format!("There is no profile named \"{name}\".")
}

/// `name` without surrounding spaces, if it can be a new profile next to `existing`.
pub fn check_name(name: &str, existing: &[String]) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(EMPTY_NAME.to_owned());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "A profile name can be at most {MAX_NAME_CHARS} characters long."
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(CONTROL_CHARACTERS.to_owned());
    }
    if name.eq_ignore_ascii_case("default") {
        return Err(RESERVED_NAME.to_owned());
    }
    // LACT tells names apart by case; two that differ only by case would confuse the user.
    let lower = name.to_lowercase();
    if existing.iter().any(|other| other.to_lowercase() == lower) {
        return Err(format!("A profile named \"{name}\" already exists."));
    }
    Ok(name.to_owned())
}

/// Switches back to the profile that was active before a pending switch.
#[derive(Debug, Clone)]
pub(super) struct Undo {
    previous: Option<String>,
}

impl Undo {
    /// Switches back and emits [`PROFILE_EVENT`]. Called with the control lock held.
    pub(super) fn run(&self, tuning: &Tuning) -> Result<(), String> {
        self.restore(tuning)?;
        tuning.announce(None);
        Ok(())
    }

    /// Switches back without reading the profiles again: one request, for when the app quits
    /// and no window is left to tell. Called with the control lock held.
    pub(super) fn restore(&self, tuning: &Tuning) -> Result<(), String> {
        tuning
            .inner
            .lact
            .client
            .set_profile(self.previous.as_deref())
            .map_err(|error| tuning.profile_refusal(&error))
    }
}

impl Tuning {
    /// The profiles LACT stores. Blocking.
    pub fn profiles(&self) -> Result<Profiles, String> {
        let lact = &self.inner.lact;
        lact.client
            .list_profiles()
            .map(Profiles::from)
            .map_err(|error| lact.reason(&error))
    }

    /// Stores a new profile copied from the active configuration (the active profile, or LACT's
    /// default one). Nothing is applied to the GPU. Blocking.
    pub fn create_profile(&self, name: &str) -> Result<Profiles, String> {
        let _control = lock(&self.inner.control);
        self.refuse_while_pending()?;
        let info = self.profiles()?;
        let name = check_name(name, &info.profiles)?;
        let base = match &info.active {
            Some(active) => ProfileBase::Profile(active.clone()),
            None => ProfileBase::Default,
        };
        self.inner
            .lact
            .client
            .create_profile(&name, &base)
            .map_err(|error| self.profile_refusal(&error))?;
        let mut expected = info;
        expected.profiles.push(name);
        expected.profiles.sort();
        Ok(self.announce(Some(expected.clone())).unwrap_or(expected))
    }

    /// Activates `name` (`None` = LACT's default profile) on every GPU, pending the user's
    /// answer on `gpu_id`: `tuning_confirm` keeps it, `tuning_revert` or the deadline switches
    /// back. Blocking.
    pub fn activate_profile(
        &self,
        gpu_id: &str,
        name: Option<&str>,
    ) -> Result<PendingChange, String> {
        let _control = lock(&self.inner.control);
        self.refuse_if_pending(gpu_id)?;
        let lact = &self.inner.lact;
        lact.device_id(gpu_id)?;
        let info = self.profiles()?;
        if info.auto_switch {
            return Err(AUTO_SWITCH_ON.to_owned());
        }
        if let Some(name) = name
            && !info.profiles.iter().any(|profile| profile == name)
        {
            return Err(no_such_profile(name));
        }
        if info.active.as_deref() == name {
            return Err(ALREADY_ACTIVE.to_owned());
        }

        let previous = info.active;
        if let Err(error) = lact.client.set_profile(name) {
            // lactd 0.10.1 marks the new profile active before applying it: if applying failed
            // half way, put the previous one back.
            let moved = lact
                .client
                .list_profiles()
                .is_ok_and(|now| now.current_profile != previous);
            if moved && let Err(undo) = lact.client.set_profile(previous.as_deref()) {
                eprintln!("redline: could not restore the previous LACT profile ({undo})");
            }
            return Err(self.profile_refusal(&error));
        }
        let change = self.begin_pending(
            gpu_id,
            TuningChange::Profile,
            CONFIRM_SECONDS * 1000,
            Some(Undo { previous }),
        );
        self.announce(None);
        Ok(change)
    }

    /// Deletes a profile that is not active. Blocking.
    pub fn delete_profile(&self, name: &str) -> Result<Profiles, String> {
        let _control = lock(&self.inner.control);
        self.refuse_while_pending()?;
        let info = self.profiles()?;
        if !info.profiles.iter().any(|profile| profile == name) {
            return Err(no_such_profile(name));
        }
        if info.active.as_deref() == Some(name) {
            return Err(ACTIVE_PROFILE.to_owned());
        }
        self.inner
            .lact
            .client
            .delete_profile(name)
            .map_err(|error| self.profile_refusal(&error))?;
        let mut expected = info;
        expected.profiles.retain(|profile| profile != name);
        Ok(self.announce(Some(expected.clone())).unwrap_or(expected))
    }

    /// Refuses a profile change while any change is pending. Called with the control lock held.
    fn refuse_while_pending(&self) -> Result<(), String> {
        self.expire_overdue();
        match lock(&self.inner.state).pending {
            Some(_) => Err(PENDING_CHANGE.to_owned()),
            None => Ok(()),
        }
    }

    /// Reads the profiles after a change and emits [`PROFILE_EVENT`] with them; `expected`
    /// stands in when lactd cannot be read, and without either nothing is emitted.
    fn announce(&self, expected: Option<Profiles>) -> Option<Profiles> {
        let profiles = self.profiles().ok().or(expected)?;
        self.inner.events.profiles(&profiles);
        Some(profiles)
    }

    /// The sentence for a refused profile request.
    fn profile_refusal(&self, error: &LactError) -> String {
        match error {
            LactError::Daemon(message)
                if message.contains("hold is active") || message.contains("while it is held") =>
            {
                PROFILE_HELD.to_owned()
            }
            LactError::Daemon(message) if message.contains("already exists") => {
                "A profile with this name already exists.".to_owned()
            }
            LactError::Daemon(message) if message.contains("not found") => {
                "LACT no longer has this profile.".to_owned()
            }
            LactError::Daemon(message) => {
                format!("LACT did not change the profiles: {message}")
            }
            other => self.inner.lact.reason(other),
        }
    }
}

/// The profiles LACT stores, the active one and whether LACT switches them by itself.
#[tauri::command]
pub async fn list_profiles(tuning: State<'_, Tuning>) -> Result<Profiles, String> {
    blocking(&tuning, |tuning| tuning.profiles()).await?
}

/// Stores a new profile copied from the active configuration. Applies nothing.
#[tauri::command]
pub async fn create_profile(name: String, tuning: State<'_, Tuning>) -> Result<Profiles, String> {
    blocking(&tuning, move |tuning| tuning.create_profile(&name)).await?
}

/// Activates the profile `name` (`null` = LACT's default profile). The switch is pending on
/// `gpu_id`: the user must confirm it with `tuning_confirm` before its deadline.
#[tauri::command]
pub async fn activate_profile(
    gpu_id: String,
    name: Option<String>,
    tuning: State<'_, Tuning>,
) -> Result<PendingChange, String> {
    blocking(&tuning, move |tuning| {
        tuning.activate_profile(&gpu_id, name.as_deref())
    })
    .await?
}

/// Deletes a profile that is not active.
#[tauri::command]
pub async fn delete_profile(name: String, tuning: State<'_, Tuning>) -> Result<Profiles, String> {
    blocking(&tuning, move |tuning| tuning.delete_profile(&name)).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tuning::tests::{Collected, FakeLactd, Handler, ManualClock, Setup};
    use crate::tuning::{Clock, Job, SETTLED_EVENT, TuningChange};
    use crate::tuning::{NOTHING_PENDING, Outcome, PENDING_ELSEWHERE, PENDING_EVENT, PENDING_HERE};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    const GPU_A: &str = "1002:164e-1043:8877-0000:0c:00.0";
    const GPU_B: &str = "1002:7550-1043:0643-0000:0d:00.0";
    const SLOT_A: &str = "0000:0c:00.0";
    const SLOT_B: &str = "0000:0d:00.0";

    /// lactd 0.10.1's profile store: the names, the active one, automatic switching, a hold
    /// held by another program, and every request that changes something.
    struct Store {
        profiles: Vec<String>,
        active: Option<String>,
        auto_switch: bool,
        held: bool,
        /// `set_profile` fails after marking the new profile active, as a failed apply does.
        apply_fails: bool,
        log: Vec<Value>,
    }

    impl Default for Store {
        fn default() -> Self {
            Store {
                profiles: vec!["Quiet".into(), "Gaming".into()],
                active: Some("Quiet".into()),
                auto_switch: false,
                held: false,
                apply_fails: false,
                log: Vec::new(),
            }
        }
    }

    fn error(message: impl Into<String>) -> Value {
        json!({ "error": message.into() })
    }

    /// `set_profile`, `create_profile`, `delete_profile` and `list_profiles` as handler.rs
    /// implements them in 0.10.1.
    fn profiles_lactd(store: Arc<Mutex<Store>>) -> Box<Handler> {
        Box::new(move |command, args| {
            let mut store = store.lock().unwrap();
            match command {
                "system_info" => json!({ "version": "0.10.1" }),
                "device_info" => json!({ "driver": "amdgpu", "flags": [] }),
                "device_stats" => json!({ "fan": {}, "power": {} }),
                "list_devices" => json!([
                    { "id": GPU_A, "device_type": "Integrated" },
                    { "id": GPU_B, "device_type": "Dedicated" }
                ]),
                "list_profiles" => {
                    let profiles: serde_json::Map<String, Value> = store
                        .profiles
                        .iter()
                        .map(|name| (name.clone(), Value::Null))
                        .collect();
                    json!({
                        "profiles": profiles, "profile_hooks": {},
                        "current_profile": store.active, "auto_switch": store.auto_switch,
                        "watcher_state": null
                    })
                }
                "set_profile" => {
                    store.log.push(json!({ "set_profile": args }));
                    if store.held {
                        return error("Cannot change profile while a hold is active");
                    }
                    let name = args["name"].as_str().map(str::to_owned);
                    if let Some(name) = &name
                        && !store.profiles.contains(name)
                    {
                        return error(format!("Profile {name} not found"));
                    }
                    store.active = name;
                    store.auto_switch = args["auto_switch"].as_bool().unwrap_or(false);
                    if store.apply_fails {
                        return error("Could not apply settings");
                    }
                    Value::Null
                }
                "create_profile" => {
                    store.log.push(json!({ "create_profile": args }));
                    let name = args["name"].as_str().unwrap().to_owned();
                    if store.profiles.contains(&name) {
                        return error(format!("Profile {name} already exists"));
                    }
                    store.profiles.push(name);
                    Value::Null
                }
                "delete_profile" => {
                    store.log.push(json!({ "delete_profile": args }));
                    let name = args["name"].as_str().unwrap();
                    if store.held {
                        return error(format!("Cannot delete profile '{name}' while it is held"));
                    }
                    if store.active.as_deref() == Some(name) {
                        store.active = None;
                    }
                    store.profiles.retain(|profile| profile != name);
                    Value::Null
                }
                "confirm_pending_config" => {
                    store.log.push(json!({ "confirm_pending_config": args }));
                    error("No pending config changes")
                }
                _ => error(format!("unexpected {command}")),
            }
        })
    }

    struct Service {
        _setup: Setup,
        store: Arc<Mutex<Store>>,
        clock: Arc<ManualClock>,
        events: Arc<Collected>,
        tuning: Tuning,
        _lactd: FakeLactd,
    }

    impl Service {
        fn start(name: &str) -> Self {
            Self::with(name, Store::default())
        }

        fn with(name: &str, store: Store) -> Self {
            let setup = Setup::new(&format!("profiles-{name}"));
            let store = Arc::new(Mutex::new(store));
            let lactd = FakeLactd::start(&setup.socket(), profiles_lactd(store.clone()));
            let clock = Arc::new(ManualClock::default());
            let events = Arc::new(Collected::default());
            let tuning = Tuning::new(setup.lact(true), clock.clone(), events.clone());
            Service {
                _setup: setup,
                store,
                clock,
                events,
                tuning,
                _lactd: lactd,
            }
        }

        fn active(&self) -> Option<String> {
            self.store.lock().unwrap().active.clone()
        }

        fn log(&self) -> Vec<Value> {
            std::mem::take(&mut self.store.lock().unwrap().log)
        }
    }

    fn profiles_event(names: &[&str], active: Option<&str>) -> Value {
        json!({
            "event": PROFILE_EVENT, "profiles": names, "active": active, "auto_switch": false
        })
    }

    fn pending_event(slot: &str, deadline_ms: u64) -> Value {
        json!({
            "event": PENDING_EVENT, "gpu_id": slot, "what": "profile", "deadline_ms": deadline_ms
        })
    }

    fn settled_event(slot: &str, outcome: &str) -> Value {
        json!({ "event": SETTLED_EVENT, "gpu_id": slot, "outcome": outcome })
    }

    fn set_profile(name: Option<&str>) -> Value {
        json!({ "set_profile": { "name": name, "auto_switch": false } })
    }

    #[test]
    fn lists_the_sorted_profiles_and_the_active_one() {
        let service = Service::start("list");
        assert_eq!(
            service.tuning.profiles(),
            Ok(Profiles {
                profiles: vec!["Gaming".into(), "Quiet".into()],
                active: Some("Quiet".into()),
                auto_switch: false,
            })
        );
        // tuning_status reports the same active profile.
        assert_eq!(
            service.tuning.status(SLOT_B).profile.as_deref(),
            Some("Quiet")
        );
    }

    #[test]
    fn without_lact_the_list_is_a_sentence() {
        let setup = Setup::new("profiles-missing");
        let tuning = Tuning::new(
            setup.lact(false),
            Arc::new(ManualClock::default()),
            Arc::new(Collected::default()),
        );
        let error = tuning.profiles().unwrap_err();
        assert!(error.contains("LACT is not installed"), "{error}");
        assert_eq!(tuning.create_profile("Loud").unwrap_err(), error);
    }

    #[test]
    fn a_new_profile_copies_the_active_configuration() {
        let service = Service::start("create");
        let profiles = service.tuning.create_profile("  Loud ").unwrap();
        assert_eq!(profiles.profiles, vec!["Gaming", "Loud", "Quiet"]);
        assert_eq!(
            service.log(),
            vec![json!({
                "create_profile": { "name": "Loud", "base": { "profile": "Quiet" } }
            })]
        );
        assert_eq!(
            service.events.take(),
            vec![profiles_event(&["Gaming", "Loud", "Quiet"], Some("Quiet"))]
        );

        // With LACT's default profile active, the copy is of the default profile.
        service.store.lock().unwrap().active = None;
        service.tuning.create_profile("Silent").unwrap();
        assert_eq!(
            service.log(),
            vec![json!({ "create_profile": { "name": "Silent", "base": "default" } })]
        );
        // Nothing was applied: no switch, no pending change.
        assert_eq!(service.tuning.pending(SLOT_B), None);
    }

    #[test]
    fn a_bad_name_is_refused_before_lactd() {
        let service = Service::start("names");
        let long = "x".repeat(MAX_NAME_CHARS + 1);
        for (name, expected) in [
            ("", EMPTY_NAME.to_owned()),
            ("   ", EMPTY_NAME.to_owned()),
            ("a\nb", CONTROL_CHARACTERS.to_owned()),
            ("Default", RESERVED_NAME.to_owned()),
            ("DEFAULT", RESERVED_NAME.to_owned()),
            (
                "quiet",
                "A profile named \"quiet\" already exists.".to_owned(),
            ),
            (
                long.as_str(),
                "A profile name can be at most 64 characters long.".to_owned(),
            ),
        ] {
            assert_eq!(
                service.tuning.create_profile(name),
                Err(expected),
                "{name:?}"
            );
        }
        assert_eq!(service.log(), Vec::<Value>::new());
        assert_eq!(service.events.take(), Vec::<Value>::new());
        // 64 characters, multibyte included, are fine.
        assert!(check_name(&"é".repeat(MAX_NAME_CHARS), &[]).is_ok());
    }

    #[test]
    fn a_confirmed_switch_stays_without_asking_lactd_anything() {
        let service = Service::start("confirm");
        service.clock.advance(1_000);
        let change = service
            .tuning
            .activate_profile(SLOT_B, Some("Gaming"))
            .unwrap();
        assert_eq!(
            change,
            PendingChange {
                gpu_id: SLOT_B.into(),
                what: TuningChange::Profile,
                deadline_ms: 1_000 + CONFIRM_SECONDS * 1000,
            }
        );
        assert_eq!(service.active().as_deref(), Some("Gaming"));
        assert_eq!(service.log(), vec![set_profile(Some("Gaming"))]);
        assert_eq!(
            service.events.take(),
            vec![
                pending_event(SLOT_B, 16_000),
                profiles_event(&["Gaming", "Quiet"], Some("Gaming"))
            ]
        );
        assert_eq!(service.tuning.status(SLOT_B).pending, Some(change));

        assert_eq!(service.tuning.settle(SLOT_B, true), Ok(Outcome::Confirmed));
        // lactd already saved the switch: no confirm_pending_config, no second set_profile.
        assert_eq!(service.log(), Vec::<Value>::new());
        assert_eq!(
            service.events.take(),
            vec![settled_event(SLOT_B, "confirmed")]
        );
        service.clock.advance(60_000);
        assert_eq!(service.active().as_deref(), Some("Gaming"));
        assert_eq!(service.events.take(), Vec::<Value>::new());
    }

    #[test]
    fn a_reverted_switch_goes_back_to_the_previous_profile() {
        let service = Service::start("revert");
        service.tuning.activate_profile(SLOT_B, None).unwrap();
        assert_eq!(service.active(), None);
        service.events.take();
        service.log();

        assert_eq!(service.tuning.settle(SLOT_B, false), Ok(Outcome::Reverted));
        assert_eq!(service.active().as_deref(), Some("Quiet"));
        assert_eq!(service.log(), vec![set_profile(Some("Quiet"))]);
        assert_eq!(
            service.events.take(),
            vec![
                profiles_event(&["Gaming", "Quiet"], Some("Quiet")),
                settled_event(SLOT_B, "reverted")
            ]
        );
    }

    #[test]
    fn without_an_answer_redline_switches_back_at_the_deadline() {
        let service = Service::start("expire");
        service
            .tuning
            .activate_profile(SLOT_B, Some("Gaming"))
            .unwrap();
        service.events.take();
        service.log();

        service.clock.advance(CONFIRM_SECONDS * 1000 - 1);
        assert_eq!(service.active().as_deref(), Some("Gaming"));
        service.clock.advance(1);
        assert_eq!(service.active().as_deref(), Some("Quiet"));
        assert_eq!(service.log(), vec![set_profile(Some("Quiet"))]);
        assert_eq!(
            service.events.take(),
            vec![
                profiles_event(&["Gaming", "Quiet"], Some("Quiet")),
                settled_event(SLOT_B, "expired")
            ]
        );
        // A late answer finds nothing.
        assert_eq!(
            service.tuning.settle(SLOT_B, true),
            Err(NOTHING_PENDING.to_owned())
        );
    }

    /// A clock whose timers never fire: an answer can arrive at the deadline before them.
    #[derive(Default)]
    struct StoppedClock(Mutex<u64>);

    impl Clock for StoppedClock {
        fn now_ms(&self) -> u64 {
            *self.0.lock().unwrap()
        }
        fn after(&self, _delay_ms: u64, _job: Job) {}
    }

    #[test]
    fn an_answer_at_the_deadline_switches_back_and_reports_expired() {
        let setup = Setup::new("profiles-late");
        let store = Arc::new(Mutex::new(Store::default()));
        let _lactd = FakeLactd::start(&setup.socket(), profiles_lactd(store.clone()));
        let clock = Arc::new(StoppedClock::default());
        let events = Arc::new(Collected::default());
        let tuning = Tuning::new(setup.lact(true), clock.clone(), events.clone());
        tuning.activate_profile(SLOT_B, Some("Gaming")).unwrap();
        events.take();

        *clock.0.lock().unwrap() = CONFIRM_SECONDS * 1000;
        assert_eq!(tuning.settle(SLOT_B, true), Ok(Outcome::Expired));
        assert_eq!(store.lock().unwrap().active.as_deref(), Some("Quiet"));
        assert_eq!(
            events.take(),
            vec![
                profiles_event(&["Gaming", "Quiet"], Some("Quiet")),
                settled_event(SLOT_B, "expired")
            ]
        );
    }

    #[test]
    fn quitting_with_a_switch_pending_switches_back() {
        let service = Service::start("quit");
        service
            .tuning
            .activate_profile(SLOT_B, Some("Gaming"))
            .unwrap();
        service.events.take();
        service.log();
        service.tuning.shutdown();
        assert_eq!(service.active().as_deref(), Some("Quiet"));
        assert_eq!(service.log(), vec![set_profile(Some("Quiet"))]);
        // Only the settlement: the profiles are not read again on the way out.
        assert_eq!(
            service.events.take(),
            vec![settled_event(SLOT_B, "reverted")]
        );
    }

    #[test]
    fn a_failed_revert_keeps_the_switch_pending() {
        let service = Service::start("revert-fails");
        service
            .tuning
            .activate_profile(SLOT_B, Some("Gaming"))
            .unwrap();
        service.store.lock().unwrap().held = true;
        assert_eq!(
            service.tuning.settle(SLOT_B, false),
            Err(PROFILE_HELD.to_owned())
        );
        assert!(service.tuning.pending(SLOT_B).is_some());
        service.store.lock().unwrap().held = false;
        assert_eq!(service.tuning.settle(SLOT_B, false), Ok(Outcome::Reverted));
        assert_eq!(service.active().as_deref(), Some("Quiet"));
    }

    #[test]
    fn one_change_at_a_time_across_switches_and_setters() {
        let service = Service::start("one");
        service
            .tuning
            .activate_profile(SLOT_B, Some("Gaming"))
            .unwrap();
        service.log();
        assert_eq!(
            service.tuning.activate_profile(SLOT_B, None),
            Err(PENDING_HERE.to_owned())
        );
        assert_eq!(
            service.tuning.activate_profile(SLOT_A, None),
            Err(PENDING_ELSEWHERE.to_owned())
        );
        // A setter would be reverted by lactd into whichever profile is active by then.
        assert_eq!(
            service.tuning.set_power_limit(SLOT_B, Some(250.0)),
            Err(PENDING_HERE.to_owned())
        );
        // Creating or deleting would save the unconfirmed configuration to disk.
        assert_eq!(
            service.tuning.create_profile("Loud"),
            Err(PENDING_CHANGE.to_owned())
        );
        assert_eq!(
            service.tuning.delete_profile("Quiet"),
            Err(PENDING_CHANGE.to_owned())
        );
        assert_eq!(service.log(), Vec::<Value>::new());
    }

    #[test]
    fn a_switch_lact_cannot_make_is_refused_before_lactd() {
        let service = Service::with(
            "refused",
            Store {
                auto_switch: true,
                ..Store::default()
            },
        );
        assert_eq!(
            service.tuning.activate_profile(SLOT_B, Some("Gaming")),
            Err(AUTO_SWITCH_ON.to_owned())
        );
        service.store.lock().unwrap().auto_switch = false;
        assert_eq!(
            service.tuning.activate_profile(SLOT_B, Some("Loud")),
            Err("There is no profile named \"Loud\".".to_owned())
        );
        assert_eq!(
            service.tuning.activate_profile(SLOT_B, Some("Quiet")),
            Err(ALREADY_ACTIVE.to_owned())
        );
        assert_eq!(
            service
                .tuning
                .activate_profile("0000:0e:00.0", Some("Gaming")),
            Err(crate::tuning::NOT_MANAGED.to_owned())
        );
        assert_eq!(service.log(), Vec::<Value>::new());
        assert_eq!(service.tuning.pending(SLOT_B), None);
    }

    #[test]
    fn a_refused_switch_is_a_sentence_and_nothing_is_pending() {
        let service = Service::start("held");
        service.store.lock().unwrap().held = true;
        assert_eq!(
            service.tuning.activate_profile(SLOT_B, Some("Gaming")),
            Err(PROFILE_HELD.to_owned())
        );
        assert_eq!(service.tuning.pending(SLOT_B), None);
        assert_eq!(service.events.take(), Vec::<Value>::new());
    }

    #[test]
    fn a_switch_that_fails_half_way_puts_the_previous_profile_back() {
        let service = Service::start("half");
        service.store.lock().unwrap().apply_fails = true;
        assert_eq!(
            service.tuning.activate_profile(SLOT_B, Some("Gaming")),
            Err("LACT did not change the profiles: Could not apply settings".to_owned())
        );
        assert_eq!(
            service.log(),
            vec![set_profile(Some("Gaming")), set_profile(Some("Quiet"))]
        );
        assert_eq!(service.active().as_deref(), Some("Quiet"));
        assert_eq!(service.tuning.pending(SLOT_B), None);
    }

    #[test]
    fn only_a_profile_that_is_not_active_can_be_deleted() {
        let service = Service::start("delete");
        assert_eq!(
            service.tuning.delete_profile("Quiet"),
            Err(ACTIVE_PROFILE.to_owned())
        );
        assert_eq!(
            service.tuning.delete_profile("Loud"),
            Err("There is no profile named \"Loud\".".to_owned())
        );
        assert_eq!(service.log(), Vec::<Value>::new());

        let profiles = service.tuning.delete_profile("Gaming").unwrap();
        assert_eq!(profiles.profiles, vec!["Quiet"]);
        assert_eq!(
            service.log(),
            vec![json!({ "delete_profile": { "name": "Gaming" } })]
        );
        assert_eq!(
            service.events.take(),
            vec![profiles_event(&["Quiet"], Some("Quiet"))]
        );
        assert_eq!(service.active().as_deref(), Some("Quiet"));

        service.store.lock().unwrap().profiles.push("Gaming".into());
        service.store.lock().unwrap().held = true;
        assert_eq!(
            service.tuning.delete_profile("Gaming"),
            Err(PROFILE_HELD.to_owned())
        );
    }
}
