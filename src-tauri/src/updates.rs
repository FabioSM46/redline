//! The check for a newer release (D14): a background thread, its status and its commands. The
//! client and the comparison are in `redline_core::update`.
//!
//! - **Never blocks startup.** The thread starts in `setup`, checks [`FIRST_CHECK`] later and
//!   every [`CHECK_EVERY`] while the app runs. With `check_for_updates` off a round makes no
//!   request; turning it on checks at once ([`check_now`]) and turning it off forgets what the
//!   checks found ([`forget`]), so no notice outlives the opt-out.
//! - **A result from before the opt-out is dropped.** [`Updates::finish`] tests the setting and a
//!   generation that `forget` moves on, under the lock that holds the write and the event, so a
//!   check still running when the setting goes off writes and sends nothing, and a result that
//!   was first reaches the frontend before the event of `forget`.
//! - **A failed check is an outcome.** `update_status` and [`STATUS_EVENT`] report it in
//!   `last_check`, the latest known version survives it, and the next try is the next round.
//!   Only `unreachable` gets one more check, [`RETRY_AFTER`] later: it does not move the rounds,
//!   a failed retry gets none, and with the setting off it requests nothing. Turning the setting
//!   on replaces it with the check that starts at once. A rate limit, a bad answer or a missing
//!   `curl` would not change in that time. `unreachable` means no answer reached Redline, not
//!   that GitHub saw nothing: a timeout can come after the request left.
//! - **`update_status()`** → `UpdateStatus { current_version, latest_version, update_available,
//!   dismissed, last_check: "never" | "ok" | "curl_missing" | "unreachable" | "refused" |
//!   "bad_response" }`. `latest_version` is `null` until a check succeeds; the latest version is
//!   kept in memory only. `update_available` ignores the dismissal: it is `dismissed` that says
//!   whether the version was set aside. [`STATUS_EVENT`] carries the same object when it changes.
//! - **`dismiss_update()`** saves the latest known version in `dismissed_update`, so a later
//!   version reports again.
//! - **`open_latest_release()`** opens the constant [`RELEASE_PAGE`], nothing read from the
//!   network.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use redline_core::update::{self, CheckFailure, KILL_AFTER, RELEASE_PAGE, Version};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::gallery;
use crate::settings::{self, SettingsState};

/// Sent with an [`UpdateStatus`] whenever it changes.
pub const STATUS_EVENT: &str = "update-status";
/// Late enough for an autostarted app to find the network up.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// For a network that was not ready at [`FIRST_CHECK`].
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// How the last check ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LastCheck {
    /// None has finished since the start or since the setting was last on.
    Never,
    Ok,
    CurlMissing,
    Unreachable,
    Refused,
    BadResponse,
}

impl From<CheckFailure> for LastCheck {
    fn from(failure: CheckFailure) -> Self {
        match failure {
            CheckFailure::CurlMissing => Self::CurlMissing,
            CheckFailure::Unreachable => Self::Unreachable,
            CheckFailure::Refused => Self::Refused,
            CheckFailure::BadResponse => Self::BadResponse,
        }
    }
}

/// What `update_status` returns and [`STATUS_EVENT`] carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateStatus {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub dismissed: bool,
    pub last_check: LastCheck,
}

struct Known {
    latest: Option<Version>,
    last_check: LastCheck,
    /// Moved on by every `forget`: a check started in an older one lost its right to be recorded.
    generation: u64,
    check_now: bool,
    /// Ends the thread of a test. The app's thread ends with the process.
    stopped: bool,
}

impl Known {
    /// A failure keeps the latest known version.
    fn apply(&mut self, result: Result<Version, CheckFailure>) {
        match result {
            Ok(latest) => {
                self.latest = Some(latest);
                self.last_check = LastCheck::Ok;
            }
            Err(failure) => self.last_check = failure.into(),
        }
    }
}

/// The managed state: the running version and what the checks have found.
pub struct Updates {
    current: String,
    known: Mutex<Known>,
    wake: Condvar,
}

impl Updates {
    fn new(current: String) -> Self {
        Self {
            current,
            known: Mutex::new(Known {
                latest: None,
                last_check: LastCheck::Never,
                generation: 0,
                check_now: false,
                stopped: false,
            }),
            wake: Condvar::new(),
        }
    }

    fn known(&self) -> MutexGuard<'_, Known> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn latest(&self) -> Option<Version> {
        self.known().latest
    }

    fn generation(&self) -> u64 {
        self.known().generation
    }

    /// The status, with `dismissed` the version stored in the settings.
    fn status(&self, dismissed: Option<&str>) -> UpdateStatus {
        self.status_of(&self.known(), dismissed)
    }

    fn status_of(&self, known: &Known, dismissed: Option<&str>) -> UpdateStatus {
        UpdateStatus {
            current_version: self.current.clone(),
            latest_version: known.latest.map(|latest| latest.to_string()),
            update_available: known
                .latest
                .is_some_and(|latest| update::is_update(&self.current, latest)),
            dismissed: known
                .latest
                .is_some_and(|latest| dismissed.and_then(Version::parse) == Some(latest)),
            last_check: known.last_check,
        }
    }

    /// Records a finished check and publishes the status if it differs from `before`, unless the
    /// setting is off or a `forget` ran since `generation` was read. `true` when a recorded check
    /// ended as `unreachable`.
    ///
    /// All of it holds the lock. The setting goes off before `forget` runs, so a result that
    /// passes the test here is either forgotten by a `forget` that waits for this lock, with its
    /// event after this one, or not touched at all.
    ///
    /// Two rules keep this free of deadlocks, and nothing but this note enforces them. The lock
    /// order is `known`, then the settings lock: no code may reach `known` with a settings guard
    /// alive, and `set_settings` drops its guard before `forget` for that reason. `publish` must
    /// not wait for the main thread, where commands that take this lock run: Tauri's emit from
    /// another thread only queues a script, unless its `tracing` feature is on, which makes it
    /// wait for the answer. Recheck this on a Tauri update or a new feature flag.
    fn finish(
        &self,
        generation: u64,
        result: Result<Version, CheckFailure>,
        dismissed: Option<&str>,
        before: &UpdateStatus,
        host: &impl Host,
    ) -> bool {
        let mut known = self.known();
        if known.generation != generation || !host.enabled() {
            return false;
        }
        known.apply(result);
        let after = self.status_of(&known, dismissed);
        if after != *before {
            host.publish(&after);
        }
        result == Err(CheckFailure::Unreachable)
    }

    /// Back to the state before any check. `true` when something was known.
    fn forget(&self) -> bool {
        let mut known = self.known();
        let changed = known.latest.is_some() || known.last_check != LastCheck::Never;
        known.latest = None;
        known.last_check = LastCheck::Never;
        known.generation = known.generation.wrapping_add(1);
        changed
    }

    fn request_check(&self) {
        self.known().check_now = true;
        self.wake.notify_all();
    }

    /// Sleeps until `next`, `retry` or a [`Self::request_check`]. A regular turn moves `next` on
    /// by `every` and replaces a waiting retry; a retry leaves `next` alone. `None` when the
    /// thread must end.
    fn wait_turn(
        &self,
        next: &mut Instant,
        retry: &mut Option<Instant>,
        every: Duration,
    ) -> Option<Turn> {
        let mut known = self.known();
        loop {
            if known.stopped {
                return None;
            }
            let now = Instant::now();
            if known.check_now || now >= *next {
                known.check_now = false;
                *next = now + every;
                *retry = None;
                return Some(Turn::Regular);
            }
            if retry.is_some_and(|at| now >= at) {
                *retry = None;
                return Some(Turn::Retry);
            }
            let until = retry.map_or(*next, |at| at.min(*next));
            known = self
                .wake
                .wait_timeout(known, until - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Turn {
    Regular,
    Retry,
}

/// What the thread needs from the app.
trait Host {
    fn enabled(&self) -> bool;
    fn dismissed(&self) -> Option<String>;
    fn fetch(&self) -> Result<Version, CheckFailure>;
    fn publish(&self, status: &UpdateStatus);
}

/// One check, unless the setting is off. An answer that arrives after the setting was turned off
/// is dropped, see [`Updates::finish`]. Publishes only a status that changed. `true` when the
/// check counts as `unreachable`.
fn round(updates: &Updates, host: &impl Host) -> bool {
    // Before the test of the setting: a `forget` after this reading drops the result, and one
    // before it comes after the setting went off, so the test stops the round.
    let generation = updates.generation();
    if !host.enabled() {
        return false;
    }
    let dismissed = host.dismissed();
    let before = updates.status(dismissed.as_deref());
    let result = host.fetch();
    updates.finish(generation, result, dismissed.as_deref(), &before, host)
}

fn run(
    updates: &Updates,
    host: &impl Host,
    first: Duration,
    every: Duration,
    retry_after: Duration,
) {
    let mut next = Instant::now() + first;
    let mut retry = None;
    while let Some(turn) = updates.wait_turn(&mut next, &mut retry, every) {
        if round(updates, host) && turn == Turn::Regular {
            retry = Some(Instant::now() + retry_after);
        }
    }
}

fn emit<R: Runtime>(app: &AppHandle<R>, status: &UpdateStatus) {
    if let Err(error) = app.emit(STATUS_EVENT, status) {
        eprintln!("redline: could not send {STATUS_EVENT} ({error})");
    }
}

struct AppHost<R: Runtime>(AppHandle<R>);

impl<R: Runtime> AppHost<R> {
    fn settings(&self) -> Option<settings::Settings> {
        self.0
            .try_state::<SettingsState>()
            .map(|state| state.current())
    }
}

impl<R: Runtime> Host for AppHost<R> {
    fn enabled(&self) -> bool {
        self.settings()
            .is_some_and(|settings| settings.check_for_updates)
    }

    fn dismissed(&self) -> Option<String> {
        self.settings()?.dismissed_update
    }

    fn fetch(&self) -> Result<Version, CheckFailure> {
        let search_path = std::env::var_os("PATH").unwrap_or_default();
        update::latest_release(&search_path, KILL_AFTER)
    }

    fn publish(&self, status: &UpdateStatus) {
        emit(&self.0, status);
    }
}

/// Registers the state and starts the thread. Call after [`settings::init`].
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let updates = Arc::new(Updates::new(app.package_info().version.to_string()));
    app.manage(updates.clone());
    let host = AppHost(app.clone());
    let started = thread::Builder::new()
        .name("redline-updates".into())
        .spawn(move || run(&updates, &host, FIRST_CHECK, CHECK_EVERY, RETRY_AFTER));
    if let Err(error) = started {
        eprintln!("redline: could not start the update check ({error})");
    }
}

/// `check_for_updates` was turned on: checks without waiting for the next round.
pub fn check_now<R: Runtime>(app: &AppHandle<R>) {
    if let Some(updates) = app.try_state::<Arc<Updates>>() {
        updates.request_check();
    }
}

/// `check_for_updates` was turned off: forgets what the checks found and says so.
pub fn forget<R: Runtime>(app: &AppHandle<R>) {
    let Some(updates) = app.try_state::<Arc<Updates>>() else {
        return;
    };
    if updates.forget() {
        let dismissed = AppHost(app.clone()).dismissed();
        emit(app, &updates.status(dismissed.as_deref()));
    }
}

/// See the module documentation.
#[tauri::command]
pub fn update_status(
    updates: State<'_, Arc<Updates>>,
    settings: State<'_, SettingsState>,
) -> UpdateStatus {
    updates.status(settings.current().dismissed_update.as_deref())
}

/// Dismisses the latest known version; without one there is nothing to dismiss.
#[tauri::command]
pub fn dismiss_update<R: Runtime>(
    app: AppHandle<R>,
    updates: State<'_, Arc<Updates>>,
    settings: State<'_, SettingsState>,
) -> Result<UpdateStatus, String> {
    let before = updates.status(settings.current().dismissed_update.as_deref());
    if let Some(latest) = updates.latest() {
        settings::save_dismissed_update(&settings, latest.to_string())?;
    }
    let status = updates.status(settings.current().dismissed_update.as_deref());
    if status != before {
        emit(&app, &status);
    }
    Ok(status)
}

/// Opens the release page in the default browser.
#[tauri::command]
pub fn open_latest_release() -> Result<(), String> {
    gallery::xdg_open(RELEASE_PAGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::collections::VecDeque;
    use std::ffi::OsStr;
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const SOON: Duration = Duration::from_millis(5);
    const NEVER: Duration = Duration::from_secs(3600);
    const PATIENCE: Duration = Duration::from_secs(10);

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    struct Fake {
        enabled: AtomicBool,
        answer: Mutex<Result<Version, CheckFailure>>,
        /// Answered first, one per check, before `answer`.
        queued: Mutex<VecDeque<Result<Version, CheckFailure>>>,
        fetched_at: Mutex<Vec<Instant>>,
        fetched: AtomicUsize,
        published: Mutex<Vec<UpdateStatus>>,
        dismissed: Option<String>,
    }

    impl Fake {
        fn new(enabled: bool) -> Self {
            Fake {
                enabled: AtomicBool::new(enabled),
                answer: Mutex::new(Ok(version("0.3.0"))),
                queued: Mutex::new(VecDeque::new()),
                fetched_at: Mutex::new(Vec::new()),
                fetched: AtomicUsize::new(0),
                published: Mutex::new(Vec::new()),
                dismissed: None,
            }
        }

        fn fetched(&self) -> usize {
            self.fetched.load(Ordering::SeqCst)
        }
    }

    impl Host for Arc<Fake> {
        fn enabled(&self) -> bool {
            self.enabled.load(Ordering::SeqCst)
        }
        fn dismissed(&self) -> Option<String> {
            self.dismissed.clone()
        }
        fn fetch(&self) -> Result<Version, CheckFailure> {
            self.fetched_at.lock().unwrap().push(Instant::now());
            self.fetched.fetch_add(1, Ordering::SeqCst);
            let queued = self.queued.lock().unwrap().pop_front();
            queued.unwrap_or_else(|| *self.answer.lock().unwrap())
        }
        fn publish(&self, status: &UpdateStatus) {
            self.published.lock().unwrap().push(status.clone());
        }
    }

    impl Updates {
        fn record(&self, result: Result<Version, CheckFailure>) {
            self.known().apply(result);
        }
    }

    /// The thread, running against `host`; it is stopped and joined on drop.
    struct Running {
        updates: Arc<Updates>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Running {
        fn start(host: Arc<Fake>, first: Duration, every: Duration) -> Self {
            Self::with_retry(host, first, every, NEVER)
        }

        fn with_retry(
            host: Arc<Fake>,
            first: Duration,
            every: Duration,
            retry_after: Duration,
        ) -> Self {
            let updates = Arc::new(Updates::new("0.2.1".into()));
            let shared = updates.clone();
            let thread = thread::spawn(move || run(&shared, &host, first, every, retry_after));
            Running {
                updates,
                thread: Some(thread),
            }
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.updates.known().stopped = true;
            self.updates.wake.notify_all();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn before_any_check_nothing_is_known() {
        let updates = Updates::new("0.2.1".into());
        let status = updates.status(Some("0.3.0"));
        assert_eq!(
            status,
            UpdateStatus {
                current_version: "0.2.1".into(),
                latest_version: None,
                update_available: false,
                dismissed: false,
                last_check: LastCheck::Never,
            }
        );
    }

    #[test]
    fn a_newer_release_is_available_until_the_check_says_otherwise() {
        let updates = Updates::new("0.2.1".into());
        updates.record(Ok(version("0.3.0")));
        let status = updates.status(None);
        assert_eq!(status.latest_version.as_deref(), Some("0.3.0"));
        assert!(status.update_available);
        assert_eq!(status.last_check, LastCheck::Ok);

        for same_or_older in ["0.2.1", "0.2.0"] {
            updates.record(Ok(version(same_or_older)));
            assert!(!updates.status(None).update_available, "{same_or_older}");
        }
    }

    #[test]
    fn a_build_newer_than_the_release_reports_nothing() {
        let updates = Updates::new("0.4.0".into());
        updates.record(Ok(version("0.3.0")));
        assert!(!updates.status(None).update_available);
    }

    #[test]
    fn a_failed_check_keeps_the_latest_known_version() {
        let updates = Updates::new("0.2.1".into());
        updates.record(Ok(version("0.3.0")));
        for (failure, outcome) in [
            (CheckFailure::CurlMissing, LastCheck::CurlMissing),
            (CheckFailure::Unreachable, LastCheck::Unreachable),
            (CheckFailure::Refused, LastCheck::Refused),
            (CheckFailure::BadResponse, LastCheck::BadResponse),
        ] {
            updates.record(Err(failure));
            let status = updates.status(None);
            assert_eq!(status.last_check, outcome);
            assert_eq!(status.latest_version.as_deref(), Some("0.3.0"));
            assert!(status.update_available);
        }
    }

    #[test]
    fn a_dismissal_covers_only_its_version() {
        let updates = Updates::new("0.2.1".into());
        updates.record(Ok(version("0.3.0")));
        assert!(updates.status(Some("0.3.0")).dismissed);
        assert!(updates.status(Some("0.3.0")).update_available);
        assert!(!updates.status(None).dismissed);

        updates.record(Ok(version("0.3.1")));
        assert!(!updates.status(Some("0.3.0")).dismissed);
    }

    #[test]
    fn the_status_uses_the_names_the_frontend_reads() {
        let updates = Updates::new("0.2.1".into());
        updates.record(Err(CheckFailure::CurlMissing));
        let json = serde_json::to_value(updates.status(None)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "current_version": "0.2.1",
                "latest_version": null,
                "update_available": false,
                "dismissed": false,
                "last_check": "curl_missing",
            })
        );
    }

    #[test]
    fn checks_start_late_and_repeat_at_the_interval() {
        let host = Arc::new(Fake::new(true));
        let _running = Running::start(host.clone(), SOON, SOON);
        wait_for("three checks", || host.fetched() >= 3);
    }

    #[test]
    fn no_check_runs_before_the_first_delay() {
        let host = Arc::new(Fake::new(true));
        let _running = Running::start(host.clone(), NEVER, NEVER);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(host.fetched(), 0);
    }

    #[test]
    fn a_toggled_off_check_requests_nothing_and_turning_it_on_checks_at_once() {
        let host = Arc::new(Fake::new(false));
        let running = Running::start(host.clone(), SOON, NEVER);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(host.fetched(), 0);

        host.enabled.store(true, Ordering::SeqCst);
        running.updates.request_check();
        wait_for("the check", || host.fetched() == 1);
    }

    #[test]
    fn turning_the_check_off_stops_the_following_ones() {
        let host = Arc::new(Fake::new(true));
        let _running = Running::start(host.clone(), SOON, SOON);
        wait_for("a check", || host.fetched() >= 1);
        host.enabled.store(false, Ordering::SeqCst);
        // A round may have been past the test of the setting.
        thread::sleep(Duration::from_millis(30));
        let settled = host.fetched();
        thread::sleep(Duration::from_millis(30));
        assert_eq!(host.fetched(), settled);
    }

    #[test]
    fn only_a_change_is_published() {
        let host = Arc::new(Fake::new(true));
        let updates = Updates::new("0.2.1".into());
        round(&updates, &host);
        round(&updates, &host);
        *host.answer.lock().unwrap() = Err(CheckFailure::Refused);
        round(&updates, &host);

        let published = host.published.lock().unwrap();
        let outcomes: Vec<_> = published.iter().map(|status| status.last_check).collect();
        assert_eq!(outcomes, [LastCheck::Ok, LastCheck::Refused]);
    }

    #[test]
    fn turning_the_check_off_forgets_what_it_found() {
        let updates = Updates::new("0.2.1".into());
        assert!(!updates.forget());

        updates.record(Ok(version("0.3.0")));
        assert!(updates.forget());
        let status = updates.status(Some("0.3.0"));
        assert_eq!(status.latest_version, None);
        assert!(!status.update_available);
        assert!(!status.dismissed);
        assert_eq!(status.last_check, LastCheck::Never);

        updates.record(Err(CheckFailure::Refused));
        assert!(updates.forget());
        assert!(!updates.forget());
    }

    struct TurnedOffDuringFetch(Arc<Fake>);

    impl Host for TurnedOffDuringFetch {
        fn enabled(&self) -> bool {
            self.0.enabled()
        }
        fn dismissed(&self) -> Option<String> {
            None
        }
        fn fetch(&self) -> Result<Version, CheckFailure> {
            self.0.enabled.store(false, Ordering::SeqCst);
            self.0.fetch()
        }
        fn publish(&self, status: &UpdateStatus) {
            self.0.publish(status);
        }
    }

    #[test]
    fn an_answer_that_arrives_after_turning_off_is_dropped() {
        let host = Arc::new(Fake::new(true));
        let updates = Updates::new("0.2.1".into());
        round(&updates, &TurnedOffDuringFetch(host.clone()));
        assert_eq!(updates.status(None).latest_version, None);
        assert!(host.published.lock().unwrap().is_empty());
    }

    /// The setting goes off and back on during the fetch: it reads on, so only the generation
    /// tells that a `forget` ran.
    struct ToggledDuringFetch(Arc<Fake>, Arc<Updates>);

    impl Host for ToggledDuringFetch {
        fn enabled(&self) -> bool {
            self.0.enabled()
        }
        fn dismissed(&self) -> Option<String> {
            None
        }
        fn fetch(&self) -> Result<Version, CheckFailure> {
            self.1.forget();
            self.0.fetch()
        }
        fn publish(&self, status: &UpdateStatus) {
            self.0.publish(status);
        }
    }

    #[test]
    fn a_result_from_before_a_forget_is_dropped_with_the_setting_on() {
        let host = Arc::new(Fake::new(true));
        let updates = Arc::new(Updates::new("0.2.1".into()));
        let turned = ToggledDuringFetch(host.clone(), updates.clone());
        assert!(!round(&updates, &turned));
        assert_eq!(updates.status(None).latest_version, None);
        assert_eq!(updates.status(None).last_check, LastCheck::Never);
        assert!(host.published.lock().unwrap().is_empty());

        round(&updates, &host);
        assert_eq!(
            updates.status(None).latest_version.as_deref(),
            Some("0.3.0")
        );
    }

    /// Calls `forget` from another thread while the round publishes, as `set_settings` can.
    struct ForgotDuringPublish {
        host: Arc<Fake>,
        updates: Arc<Updates>,
        forget: Mutex<Option<thread::JoinHandle<()>>>,
        forgotten: Arc<AtomicBool>,
    }

    impl Host for ForgotDuringPublish {
        fn enabled(&self) -> bool {
            self.host.enabled()
        }
        fn dismissed(&self) -> Option<String> {
            None
        }
        fn fetch(&self) -> Result<Version, CheckFailure> {
            self.host.fetch()
        }
        fn publish(&self, status: &UpdateStatus) {
            let (updates, forgotten) = (self.updates.clone(), self.forgotten.clone());
            *self.forget.lock().unwrap() = Some(thread::spawn(move || {
                updates.forget();
                forgotten.store(true, Ordering::SeqCst);
            }));
            thread::sleep(Duration::from_millis(30));
            assert!(!self.forgotten.load(Ordering::SeqCst), "forget overtook");
            self.host.publish(status);
        }
    }

    #[test]
    fn a_forget_cannot_overtake_the_event_of_a_result() {
        let updates = Arc::new(Updates::new("0.2.1".into()));
        let host = ForgotDuringPublish {
            host: Arc::new(Fake::new(true)),
            updates: updates.clone(),
            forget: Mutex::new(None),
            forgotten: Arc::new(AtomicBool::new(false)),
        };
        round(&updates, &host);
        host.forget.lock().unwrap().take().unwrap().join().unwrap();
        assert_eq!(updates.status(None).latest_version, None);
    }

    fn settle() {
        thread::sleep(Duration::from_millis(60));
    }

    #[test]
    fn an_unreachable_check_is_retried_once() {
        let host = Arc::new(Fake::new(true));
        host.queued
            .lock()
            .unwrap()
            .push_back(Err(CheckFailure::Unreachable));
        let running = Running::with_retry(host.clone(), SOON, NEVER, SOON);
        wait_for("the retry", || {
            running.updates.status(None).last_check == LastCheck::Ok
        });
        settle();
        assert_eq!(host.fetched(), 2);
    }

    #[test]
    fn a_failed_retry_leaves_the_next_try_to_the_normal_turn() {
        let every = Duration::from_millis(300);
        let host = Arc::new(Fake::new(true));
        *host.answer.lock().unwrap() = Err(CheckFailure::Unreachable);
        let running = Running::with_retry(host.clone(), SOON, every, SOON);
        wait_for("the normal turn", || host.fetched() >= 3);
        drop(running);
        let at = host.fetched_at.lock().unwrap();
        assert!(at[2] - at[0] >= every / 2, "the third check came early");
    }

    #[test]
    fn a_retry_does_not_move_the_normal_turn() {
        let every = Duration::from_millis(400);
        let retry = Duration::from_millis(250);
        let host = Arc::new(Fake::new(true));
        host.queued
            .lock()
            .unwrap()
            .push_back(Err(CheckFailure::Unreachable));
        let running = Running::with_retry(host.clone(), SOON, every, retry);
        wait_for("the normal turn", || host.fetched() >= 3);
        drop(running);
        let at = host.fetched_at.lock().unwrap();
        assert!(at[2] - at[0] < every + retry, "the turn moved");
    }

    #[test]
    fn only_unreachable_is_retried() {
        for failure in [
            CheckFailure::CurlMissing,
            CheckFailure::Refused,
            CheckFailure::BadResponse,
        ] {
            let host = Arc::new(Fake::new(true));
            *host.answer.lock().unwrap() = Err(failure);
            let _running = Running::with_retry(host.clone(), SOON, NEVER, SOON);
            wait_for("the check", || host.fetched() == 1);
            settle();
            assert_eq!(host.fetched(), 1, "{failure:?}");
        }
    }

    const RETRY: Duration = Duration::from_millis(200);

    /// The first check ended as `unreachable` and a retry is waiting for `RETRY`.
    fn waiting_retry() -> (Arc<Fake>, Running) {
        let host = Arc::new(Fake::new(true));
        host.queued
            .lock()
            .unwrap()
            .push_back(Err(CheckFailure::Unreachable));
        let running = Running::with_retry(host.clone(), SOON, NEVER, RETRY);
        wait_for("the failure", || {
            running.updates.status(None).last_check == LastCheck::Unreachable
        });
        (host, running)
    }

    #[test]
    fn turning_the_check_off_cancels_a_waiting_retry() {
        let (host, running) = waiting_retry();
        host.enabled.store(false, Ordering::SeqCst);
        running.updates.forget();
        thread::sleep(RETRY + RETRY / 2);
        assert_eq!(host.fetched(), 1);
        assert_eq!(running.updates.status(None).last_check, LastCheck::Never);
    }

    #[test]
    fn turning_the_check_on_replaces_a_waiting_retry() {
        let (host, running) = waiting_retry();
        running.updates.request_check();
        wait_for("the check", || host.fetched() >= 2);
        thread::sleep(RETRY + RETRY / 2);
        assert_eq!(host.fetched(), 2);
    }

    #[test]
    fn a_dropped_failure_is_not_retried() {
        let host = Arc::new(Fake::new(true));
        *host.answer.lock().unwrap() = Err(CheckFailure::Unreachable);
        let updates = Updates::new("0.2.1".into());
        assert!(!round(&updates, &TurnedOffDuringFetch(host.clone())));
        host.enabled.store(true, Ordering::SeqCst);
        assert!(round(&updates, &host));
    }

    /// A fake `curl` that records every run, as the real client finds and starts it.
    struct Curl {
        dir: TempDir,
    }

    impl Curl {
        fn new(name: &str) -> Self {
            let dir = TempDir::new("updates", name);
            let path = dir.join("curl");
            let script = format!(
                "#!/bin/sh\n[ \"$1\" = --probe ] && exit 0\necho run >> '{}'\n\
                 echo '{{\"tag_name\":\"v0.3.0\"}}'\n",
                dir.join("runs").display()
            );
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            // ETXTBSY, see the same wait in `dependencies`.
            let deadline = Instant::now() + PATIENCE;
            while let Err(error) = Command::new(&path).arg("--probe").status() {
                assert_eq!(error.kind(), ErrorKind::ExecutableFileBusy);
                assert!(Instant::now() < deadline, "the fake curl stays busy");
                thread::sleep(Duration::from_millis(10));
            }
            Curl { dir }
        }

        fn runs(&self) -> usize {
            fs::read_to_string(self.dir.join("runs"))
                .map(|runs| runs.lines().count())
                .unwrap_or(0)
        }
    }

    struct CurlHost<'a>(&'a Curl, bool);

    impl Host for CurlHost<'_> {
        fn enabled(&self) -> bool {
            self.1
        }
        fn dismissed(&self) -> Option<String> {
            None
        }
        fn fetch(&self) -> Result<Version, CheckFailure> {
            let path: &OsStr = self.0.dir.as_os_str();
            update::latest_release(path, PATIENCE)
        }
        fn publish(&self, _: &UpdateStatus) {}
    }

    #[test]
    fn off_never_starts_curl_and_on_does() {
        let curl = Curl::new("toggle");
        let updates = Updates::new("0.2.1".into());
        round(&updates, &CurlHost(&curl, false));
        assert_eq!(curl.runs(), 0);
        assert_eq!(updates.status(None).last_check, LastCheck::Never);

        round(&updates, &CurlHost(&curl, true));
        assert_eq!(curl.runs(), 1);
        assert!(updates.status(None).update_available);
    }
}
