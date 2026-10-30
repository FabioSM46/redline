//! Global shortcuts through the `org.freedesktop.portal.GlobalShortcuts` portal (D5, D10).
//!
//! Where the portal exists (GNOME 48 and later, KDE Plasma), Redline registers the actions of
//! [`ACTION_ARGS`] as the portal's shortcuts and performs one when the desktop reports it
//! `Activated`, with the same code a `redline --save-replay` launch runs
//! ([`actions::dispatch`]). Where it does not (GNOME 46, no portal service, no session bus), the
//! status is [`ShortcutsStatus::Unavailable`] and Settings → Shortcuts keeps showing the
//! commands to bind in the desktop's custom shortcuts, as before.
//!
//! - **Startup is never blocked**: everything runs in a task on Tauri's async runtime, started in
//!   `setup`. The first `BindShortcuts` may wait on a desktop dialog for as long as the user
//!   leaves it open.
//! - **The session lives as long as the app**, not the window: it is kept in managed state, and
//!   hiding the window in the tray touches neither the session nor the signal streams.
//! - **Every change is an event**: [`STATUS_EVENT`] carries the new [`ShortcutsStatus`] (bound,
//!   rebound from the desktop's settings, failed); [`global_shortcuts_status`] reads it.
//! - [`global_shortcuts_configure`] opens the desktop's own dialog to change the keys
//!   (`ConfigureShortcuts`, version 2 of the portal). Redline never picks or stores a key itself.
//!
//! The portal client is `ashpd` (D10) on `zbus`'s async-io stack, which runs its own reactor
//! and works from any executor, Tauri's tokio runtime included.

use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::Session;
use ashpd::desktop::global_shortcuts::{
    BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts, NewShortcut, Shortcut,
};
use ashpd::zbus::export::futures_core::Stream;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::actions::{self, ACTION_ARGS, Action};

/// The event that carries every new [`ShortcutsStatus`].
pub const STATUS_EVENT: &str = "global-shortcuts";

/// The first portal version with `ConfigureShortcuts`.
pub const CONFIGURE_VERSION: u32 = 2;

/// Where the portal's shortcuts stand, for Settings → Shortcuts. Serialized with a `state` tag:
/// `{ "state": "ready", "version": 2, "can_configure": true, "bindings": [...] }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ShortcutsStatus {
    /// The portal is being looked for, or the desktop is asking the user to bind the keys.
    Checking,
    /// No GlobalShortcuts portal: bind the commands in the desktop's custom shortcuts (D5).
    Unavailable,
    /// The shortcuts are registered and `Activated` is listened to.
    Ready {
        /// The portal's interface version.
        version: u32,
        /// Whether the desktop's binding dialog can be opened from Redline.
        can_configure: bool,
        /// One entry per action, in [`ACTION_ARGS`] order.
        bindings: Vec<Binding>,
    },
    /// The portal is there but the session or the binding failed; the commands still work.
    Failed { error: String },
}

/// One action and the keys the desktop reports for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Binding {
    /// The action's command-line argument, the key Settings → Shortcuts already uses.
    pub arg: &'static str,
    /// The desktop's description of the keys (`Ctrl+Alt+S`), `None` when nothing is bound.
    pub trigger: Option<String>,
}

/// The portal's id of an action: its argument without the leading dashes (`save-replay`). The
/// desktop stores the user's keys under this id, so it must never change.
pub fn shortcut_id(arg: &str) -> &str {
    arg.trim_start_matches('-')
}

/// The action a portal shortcut id names; `None` for an id Redline did not register.
pub fn action_for(id: &str) -> Option<Action> {
    ACTION_ARGS
        .iter()
        .find(|accepted| shortcut_id(accepted.arg) == id)
        .map(|accepted| accepted.action)
}

/// What the desktop's dialog shows for each action.
pub fn description(action: Action) -> &'static str {
    match action {
        Action::SaveReplay => "Save replay",
        Action::ToggleReplay => "Start or stop the replay",
        Action::ToggleRecording => "Start or stop a recording",
    }
}

/// The bindings to show from what the portal reports as `(id, trigger description)`: one per
/// action in [`ACTION_ARGS`] order. Unknown ids are ignored; a missing id or an empty
/// description is `None` (nothing bound).
pub fn bindings<'a>(reported: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Binding> {
    let reported: Vec<_> = reported.into_iter().collect();
    ACTION_ARGS
        .iter()
        .map(|accepted| Binding {
            arg: accepted.arg,
            trigger: reported
                .iter()
                .find(|(id, _)| *id == shortcut_id(accepted.arg))
                .map(|(_, trigger)| trigger.trim())
                .filter(|trigger| !trigger.is_empty())
                .map(str::to_owned),
        })
        .collect()
}

/// [`ShortcutsStatus::Ready`] from the portal's version and reported shortcuts.
fn ready(version: u32, shortcuts: &[Shortcut]) -> ShortcutsStatus {
    ShortcutsStatus::Ready {
        version,
        can_configure: version >= CONFIGURE_VERSION,
        bindings: bindings(
            shortcuts
                .iter()
                .map(|shortcut| (shortcut.id(), shortcut.trigger_description())),
        ),
    }
}

/// The portal proxy and its session, kept for the app's whole life.
struct Portal {
    proxy: GlobalShortcuts,
    session: Session<GlobalShortcuts>,
}

/// The managed state: the current status and, once bound, the session.
pub struct ShortcutsState {
    status: Mutex<ShortcutsStatus>,
    portal: Mutex<Option<Arc<Portal>>>,
}

impl ShortcutsState {
    fn new() -> Self {
        Self {
            status: Mutex::new(ShortcutsStatus::Checking),
            portal: Mutex::new(None),
        }
    }

    pub fn status(&self) -> ShortcutsStatus {
        lock(&self.status).clone()
    }

    fn portal(&self) -> Option<Arc<Portal>> {
        lock(&self.portal).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether `next` may replace `current`. `Unavailable` and `Failed` are final: once the session
/// is gone, a `ShortcutsChanged` still in flight must not bring back a `Ready` the configure
/// command can no longer serve.
fn replaces(current: &ShortcutsStatus, next: &ShortcutsStatus) -> bool {
    !matches!(
        (current, next),
        (
            ShortcutsStatus::Unavailable | ShortcutsStatus::Failed { .. },
            ShortcutsStatus::Ready { .. }
        )
    )
}

/// Stores `status` and sends it to the webviews, unless the current status is final (see
/// [`replaces`]). The check, the store and the event happen under one lock, so two publishers
/// cannot interleave.
fn publish<R: Runtime>(app: &AppHandle<R>, status: ShortcutsStatus) {
    let Some(state) = app.try_state::<ShortcutsState>() else {
        return;
    };
    let mut current = lock(&state.status);
    if !replaces(&current, &status) {
        return;
    }
    *current = status;
    if let Err(error) = app.emit(STATUS_EVENT, &*current) {
        eprintln!("redline: could not send {STATUS_EVENT} ({error})");
    }
}

/// Registers the state and starts the portal session in the background. Never fails and never
/// blocks: without the portal the status becomes [`ShortcutsStatus::Unavailable`].
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    app.manage(ShortcutsState::new());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let status = match session(&app).await {
            Ok(()) => return,
            Err(Setup::Unavailable(error)) => {
                // Expected on GNOME before 48: the commands stay the way to bind keys (D5).
                eprintln!("redline: no GlobalShortcuts portal ({error}); shortcuts use commands");
                ShortcutsStatus::Unavailable
            }
            Err(Setup::Failed(error)) => {
                eprintln!("redline: the global shortcuts were not registered ({error})");
                ShortcutsStatus::Failed {
                    error: format!("The desktop did not register the shortcuts: {error}"),
                }
            }
            Err(Setup::Ended) => {
                eprintln!("redline: the desktop ended the global shortcuts session");
                if let Some(state) = app.try_state::<ShortcutsState>() {
                    *lock(&state.portal) = None;
                }
                ShortcutsStatus::Failed {
                    error: "The desktop ended the shortcuts session: restart Redline to use them again."
                        .to_owned(),
                }
            }
        };
        publish(&app, status);
    });
}

/// Why the session did not start.
enum Setup {
    /// No portal to talk to: not an error, the fallback.
    Unavailable(ashpd::Error),
    /// The portal is there but refused or failed.
    Failed(ashpd::Error),
    /// The session was bound, then the desktop stopped sending its signals.
    Ended,
}

/// Finds the portal, binds the shortcuts and then listens for `Activated` and `ShortcutsChanged`
/// until the app exits.
async fn session<R: Runtime>(app: &AppHandle<R>) -> Result<(), Setup> {
    // A host (non-sandboxed) app tells the portal its id, so the desktop can keep the keys under
    // it. That is the bundle identifier, which the `.deb`'s desktop file and the autostart entry
    // are also named after, so every way of starting Redline gives the same id. Best effort: the
    // registry is newer than the portal on some desktops, and an app the portal already
    // identified (launched from its `.desktop` file) is refused, harmlessly.
    if let Ok(app_id) = ashpd::AppID::try_from(app.config().identifier.as_str())
        && let Err(error) = ashpd::register_host_app(app_id).await
    {
        eprintln!("redline: the portal registry did not take the app id ({error})");
    }

    let proxy = GlobalShortcuts::new().await.map_err(Setup::Unavailable)?;
    // Read explicitly: without a portal service at all, the proxy above still builds and
    // assumes version 1.
    let version = proxy
        .get_property::<u32>("version")
        .await
        .map_err(|error| Setup::Unavailable(error.into()))?;

    // Subscribed before binding, so no activation between the two is lost.
    let activated = proxy.receive_activated().await.map_err(Setup::Failed)?;
    let changed = proxy
        .receive_shortcuts_changed()
        .await
        .map_err(Setup::Failed)?;

    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(Setup::Failed)?;
    let shortcuts: Vec<NewShortcut> = ACTION_ARGS
        .iter()
        .map(|accepted| NewShortcut::new(shortcut_id(accepted.arg), description(accepted.action)))
        .collect();
    let bound = proxy
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await
        .and_then(|request| request.response())
        .map_err(Setup::Failed)?;

    if let Some(state) = app.try_state::<ShortcutsState>() {
        *lock(&state.portal) = Some(Arc::new(Portal { proxy, session }));
    }
    publish(app, ready(version, bound.shortcuts()));

    let on_change = {
        let app = app.clone();
        async move {
            let mut changed = pin!(changed);
            while let Some(signal) = next(changed.as_mut()).await {
                publish(&app, ready(version, signal.shortcuts()));
            }
        }
    };
    let on_activate = {
        let app = app.clone();
        async move {
            let mut activated = pin!(activated);
            while let Some(signal) = next(activated.as_mut()).await {
                match action_for(signal.shortcut_id()) {
                    Some(action) => actions::dispatch(&app, action),
                    None => eprintln!(
                        "redline: ignored an unknown global shortcut ({})",
                        signal.shortcut_id()
                    ),
                }
            }
        }
    };
    let on_change = tauri::async_runtime::spawn(on_change);
    on_activate.await;
    // Both streams end together; a change already being published loses to the final `Failed`
    // anyway (`replaces`).
    on_change.abort();
    Err(Setup::Ended)
}

/// The next item of `stream`, `None` once it has ended.
async fn next<S: Stream>(mut stream: Pin<&mut S>) -> Option<S::Item> {
    poll_fn(|cx| stream.as_mut().poll_next(cx)).await
}

/// The portal's shortcuts as Settings → Shortcuts shows them.
#[tauri::command]
pub fn global_shortcuts_status(state: State<'_, ShortcutsState>) -> ShortcutsStatus {
    state.status()
}

/// Opens the desktop's dialog to change the keys. The new keys come back as a
/// [`STATUS_EVENT`] once the desktop reports them (`ShortcutsChanged`).
#[tauri::command]
pub async fn global_shortcuts_configure(state: State<'_, ShortcutsState>) -> Result<(), String> {
    let Some(portal) = state.portal() else {
        return Err("The desktop's shortcuts are not available.".to_owned());
    };
    if portal.proxy.version() < CONFIGURE_VERSION {
        return Err(
            "This desktop cannot open its shortcut settings from Redline: change the keys in its settings."
                .to_owned(),
        );
    }
    portal
        .proxy
        .configure_shortcuts(&portal.session, None, ConfigureShortcutsOptions::default())
        .await
        .map_err(|error| format!("The desktop did not open its shortcut settings: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_stable_portal_id() {
        assert_eq!(shortcut_id("--save-replay"), "save-replay");
        assert_eq!(shortcut_id("--toggle-replay"), "toggle-replay");
        assert_eq!(shortcut_id("--toggle-recording"), "toggle-recording");
        for accepted in ACTION_ARGS {
            let id = shortcut_id(accepted.arg);
            assert!(!id.is_empty() && !id.starts_with('-'));
            assert_eq!(action_for(id), Some(accepted.action));
        }
    }

    #[test]
    fn ids_and_descriptions_are_unique() {
        for (i, a) in ACTION_ARGS.iter().enumerate() {
            for b in &ACTION_ARGS[i + 1..] {
                assert_ne!(shortcut_id(a.arg), shortcut_id(b.arg));
                assert_ne!(description(a.action), description(b.action));
            }
        }
    }

    #[test]
    fn unknown_ids_name_no_action() {
        for unknown in [
            "",
            "--save-replay",
            "Save-Replay",
            "save_replay",
            "save-replay ",
            "toggle",
            "autostart",
        ] {
            assert_eq!(action_for(unknown), None, "{unknown:?}");
        }
    }

    #[test]
    fn bindings_follow_the_action_order_whatever_the_portal_order() {
        let shown = bindings([
            ("toggle-recording", "Ctrl+Alt+R"),
            ("save-replay", "Ctrl+Alt+S"),
            ("toggle-replay", "Ctrl+Alt+P"),
        ]);
        let args: Vec<_> = shown.iter().map(|binding| binding.arg).collect();
        let expected: Vec<_> = ACTION_ARGS.iter().map(|accepted| accepted.arg).collect();
        assert_eq!(args, expected);
        assert_eq!(shown[0].trigger.as_deref(), Some("Ctrl+Alt+S"));
        assert_eq!(shown[1].trigger.as_deref(), Some("Ctrl+Alt+P"));
        assert_eq!(shown[2].trigger.as_deref(), Some("Ctrl+Alt+R"));
    }

    #[test]
    fn missing_or_empty_triggers_are_unbound_and_unknown_ids_ignored() {
        let shown = bindings([("save-replay", "  "), ("something-else", "Ctrl+X")]);
        assert_eq!(shown.len(), ACTION_ARGS.len());
        assert!(shown.iter().all(|binding| binding.trigger.is_none()));
        assert!(bindings([]).iter().all(|binding| binding.trigger.is_none()));
    }

    #[test]
    fn configuring_needs_version_two() {
        let ShortcutsStatus::Ready { can_configure, .. } = ready(1, &[]) else {
            panic!("not ready");
        };
        assert!(!can_configure);
        let ShortcutsStatus::Ready {
            can_configure,
            version,
            bindings,
        } = ready(2, &[])
        else {
            panic!("not ready");
        };
        assert!(can_configure);
        assert_eq!(version, 2);
        assert_eq!(bindings.len(), ACTION_ARGS.len());
    }

    #[test]
    fn the_status_serializes_with_a_state_tag() {
        let json = |status: &ShortcutsStatus| serde_json::to_value(status).unwrap();
        assert_eq!(
            json(&ShortcutsStatus::Checking),
            serde_json::json!({ "state": "checking" })
        );
        assert_eq!(
            json(&ShortcutsStatus::Unavailable),
            serde_json::json!({ "state": "unavailable" })
        );
        assert_eq!(
            json(&ShortcutsStatus::Failed {
                error: "no".to_owned()
            }),
            serde_json::json!({ "state": "failed", "error": "no" })
        );
        assert_eq!(
            json(&ShortcutsStatus::Ready {
                version: 2,
                can_configure: true,
                bindings: bindings([("save-replay", "Ctrl+Alt+S")]),
            }),
            serde_json::json!({
                "state": "ready",
                "version": 2,
                "can_configure": true,
                "bindings": [
                    { "arg": "--save-replay", "trigger": "Ctrl+Alt+S" },
                    { "arg": "--toggle-replay", "trigger": null },
                    { "arg": "--toggle-recording", "trigger": null },
                ],
            })
        );
    }

    #[test]
    fn a_final_status_is_never_replaced_by_ready() {
        let ready = ready(2, &[]);
        let failed = ShortcutsStatus::Failed {
            error: "ended".to_owned(),
        };
        assert!(!replaces(&failed, &ready));
        assert!(!replaces(&ShortcutsStatus::Unavailable, &ready));
        // Everything else moves on.
        assert!(replaces(&ShortcutsStatus::Checking, &ready));
        assert!(replaces(&ready, &ready));
        assert!(replaces(&ready, &failed));
        assert!(replaces(
            &ShortcutsStatus::Checking,
            &ShortcutsStatus::Unavailable
        ));
        assert!(replaces(&failed, &failed));
    }

    #[test]
    fn only_the_bundle_identifier_is_a_valid_portal_app_id() {
        // The portal registry takes a reverse-DNS id: the binary and package name is not one.
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let identifier = config["identifier"].as_str().unwrap();
        assert!(ashpd::AppID::try_from(identifier).is_ok());
        assert!(ashpd::AppID::try_from("redline").is_err());
    }

    #[test]
    fn the_state_starts_checking_with_no_session() {
        let state = ShortcutsState::new();
        assert_eq!(state.status(), ShortcutsStatus::Checking);
        assert!(state.portal().is_none());
    }
}
