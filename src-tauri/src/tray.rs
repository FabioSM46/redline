//! Tray icon, close to tray and the app's lifetime (D8).
//!
//! At startup [`create`] builds the icon with the "Show Redline" / "Quit" menu. If that fails
//! (no AppIndicator library, for example) the reason goes to stderr and [`TrayStatus`] stays
//! off. The icon counts as available only while something on the session bus shows it.
//!
//! Close to tray is always on when the tray is available (D8, no option): [`on_window_event`]
//! hides the main window instead of closing it. Without a visible icon the close quits the app.
//! While the window is hidden a watchdog asks again every [`WATCHDOG_INTERVAL`]: if the host
//! goes away (shell restart, AppIndicator extension turned off) the window comes back, so a
//! hidden window with no icon to bring it back lasts at most one interval.
//!
//! The session bus connection is opened once, when the icon is created (setup), and kept: the
//! close path and the watchdog only make the property call, bounded by a 200 ms timeout.
//!
//! The `.deb` depends on `libayatana-appindicator3-1` without a `depends` entry in
//! `tauri.conf.json`: Tauri's bundler adds it whenever the `tray-icon` feature is on, and an
//! explicit entry would list it twice (`dpkg-deb -f <deb> Depends`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{Menu, MenuBuilder, MenuEvent};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime, Window, WindowEvent};

/// The label of the main window (the one in `tauri.conf.json`, which declares no `label`).
pub const MAIN_WINDOW: &str = "main";
const TRAY_ID: &str = "main";
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";
const TOOLTIP: &str = "Redline";
/// How often the watchdog checks that a hidden window still has a visible tray icon.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);

/// Whether the tray icon was created, and the session bus connection used to ask whether it is
/// shown. Managed state, read by [`app_status`], the close handling and the watchdog.
#[derive(Debug, Default)]
pub struct TrayStatus {
    created: AtomicBool,
    /// The shared `GDBusConnection` of the session bus, null when there is none. Its reference
    /// is kept for the life of the process and never released.
    bus: AtomicPtr<c_void>,
}

impl TrayStatus {
    /// Whether the icon exists and something shows it right now: only then can the window hide.
    /// The host is asked on every call, not once at startup: the shell can register it after an
    /// autostarted Redline, or go away while it runs.
    pub fn available(&self) -> bool {
        if !self.created() {
            return false;
        }
        let bus = self.bus.load(Ordering::Acquire);
        !bus.is_null() && linux::status_notifier_host_registered(bus)
    }

    fn created(&self) -> bool {
        self.created.load(Ordering::Relaxed)
    }

    fn set_created(&self, created: bool) {
        self.created.store(created, Ordering::Relaxed);
    }
}

/// Whether the tray is available in this session, for the interface (Settings → General).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AppStatus {
    pub tray_available: bool,
}

/// The app's lifetime state for the interface: `{ "tray_available": bool }`, read at every call.
/// The session bus query is a blocking FFI call: it runs on the blocking pool, never on the main
/// thread or an async worker.
#[tauri::command]
pub async fn app_status(app: AppHandle) -> AppStatus {
    let tray_available = tauri::async_runtime::spawn_blocking(move || tray_available(&app))
        .await
        .unwrap_or(false);
    AppStatus { tray_available }
}

/// Whether the tray icon exists and is shown right now; `false` before [`create`] has run.
pub fn tray_available<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<TrayStatus>()
        .is_some_and(|status| status.available())
}

/// Registers [`TrayStatus`], then creates the tray icon, records whether it exists and starts
/// the watchdog. Never fails: without an icon the app works as before, with the window's close
/// quitting it.
pub fn create<R: Runtime>(app: &AppHandle<R>) {
    app.manage(TrayStatus::default());
    let created = match build(app) {
        Ok(()) => true,
        Err(error) => {
            // On stderr: the app has no log file, and the interface learns about the missing
            // icon through `app_status`.
            eprintln!(
                "redline: the tray icon is not available ({error}); closing the window will quit the app"
            );
            false
        }
    };
    let status = app.state::<TrayStatus>();
    status.set_created(created);
    if !created {
        return;
    }
    // The only connect, at startup: AppIndicator has just asked GIO for the same shared
    // connection, so this normally returns at once. Null means "never available".
    status.bus.store(linux::session_bus(), Ordering::Release);
    spawn_watchdog(app.clone());
}

/// Every [`WATCHDOG_INTERVAL`], brings the hidden main window back if the tray icon is no longer
/// shown. Runs for the life of the process; only started when the icon exists.
fn spawn_watchdog<R: Runtime>(app: AppHandle<R>) {
    let spawned = std::thread::Builder::new()
        .name("redline-tray-watchdog".into())
        .spawn(move || {
            loop {
                std::thread::sleep(WATCHDOG_INTERVAL);
                let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
                    continue;
                };
                // Unreadable visibility counts as visible: nothing to restore.
                let visible = window.is_visible().unwrap_or(true);
                if restore_hidden_window(visible, || tray_available(&app)) {
                    show_main_window(&app);
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("redline: the tray watchdog did not start ({error})");
    }
}

/// Whether the watchdog shows the main window again: only when it is hidden and the tray icon
/// is not shown. The tray is asked only for a hidden window.
pub fn restore_hidden_window(visible: bool, tray_available: impl FnOnce() -> bool) -> bool {
    !visible && !tray_available()
}

fn build<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    // tray-icon loads AppIndicator at runtime and panics when it is missing; with the release
    // profile's `panic = "abort"` that would kill the app at startup. Ask the dynamic loader
    // first.
    if !linux::appindicator_loads() {
        return Err("the AppIndicator library (libayatana-appindicator3) is not installed".into());
    }
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "the app has no icon".to_owned())?;
    let menu = menu(app).map_err(|error| error.to_string())?;
    // AppIndicator always opens the menu on click and reports no click events: the menu is the
    // only way in, so no click handler.
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(TOOLTIP)
        .menu(&menu)
        .on_menu_event(on_menu_event)
        .build(app)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    MenuBuilder::new(app)
        .text(MENU_SHOW, "Show Redline")
        .separator()
        .text(MENU_QUIT, "Quit")
        .build()
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        MENU_SHOW => show_main_window(app),
        // Quit always exits, whatever the window state.
        MENU_QUIT => app.exit(0),
        _ => {}
    }
}

/// Shows, unminimises and focuses the main window. Errors are ignored: there is nobody to
/// report them to, and the window stays as it was.
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Window event handler (`Builder::on_window_event`): closing the main window hides it when
/// the tray is available.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    // The label is checked first so the session bus is asked only for the main window.
    if window.label() != MAIN_WINDOW {
        return;
    }
    let tray_available = tray_available(window.app_handle());
    // A window that does not hide is closed: never a window that cannot be closed.
    if hide_on_close(window.label(), tray_available) && window.hide().is_ok() {
        api.prevent_close();
    }
}

/// Whether a close request hides the window instead of closing it: only the main window, and
/// only with a visible tray icon (D8: always on, no option). Without the icon the close quits
/// the app, or the window could not be reopened.
pub fn hide_on_close(window_label: &str, tray_available: bool) -> bool {
    window_label == MAIN_WINDOW && tray_available
}

/// Calls into system libraries the process already loads: `dlopen` (glibc, which tray-icon also
/// uses for AppIndicator) and GIO/GLib (linked by GTK). No extra crate.
mod linux {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::ptr;

    /// The same names, in the same order, that `libappindicator-sys` tries.
    const APPINDICATOR_LIBRARIES: [&CStr; 2] =
        [c"libayatana-appindicator3.so.1", c"libappindicator3.so.1"];
    /// `RTLD_LAZY | RTLD_LOCAL` (= 1, since `RTLD_LOCAL` is 0), as `libloading::Library::new`.
    const DLOPEN_FLAGS: c_int = 1;
    const G_BUS_TYPE_SESSION: c_int = 2;
    const G_DBUS_CALL_FLAGS_NONE: c_int = 0;
    const DBUS_TIMEOUT_MS: c_int = 200;

    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn g_bus_get_sync(
            bus_type: c_int,
            cancellable: *mut c_void,
            error: *mut *mut c_void,
        ) -> *mut c_void;
        #[allow(clippy::too_many_arguments)]
        fn g_dbus_connection_call_sync(
            connection: *mut c_void,
            bus_name: *const c_char,
            object_path: *const c_char,
            interface_name: *const c_char,
            method_name: *const c_char,
            parameters: *mut c_void,
            reply_type: *const c_void,
            flags: c_int,
            timeout_msec: c_int,
            cancellable: *mut c_void,
            error: *mut *mut c_void,
        ) -> *mut c_void;
        fn g_variant_new_string(string: *const c_char) -> *mut c_void;
        fn g_variant_new_tuple(children: *const *mut c_void, n_children: usize) -> *mut c_void;
        fn g_variant_get_child_value(value: *mut c_void, index: usize) -> *mut c_void;
        fn g_variant_get_variant(value: *mut c_void) -> *mut c_void;
        fn g_variant_is_of_type(value: *mut c_void, variant_type: *const c_void) -> c_int;
        fn g_variant_get_boolean(value: *mut c_void) -> c_int;
        fn g_variant_unref(value: *mut c_void);
    }

    /// Whether one of the AppIndicator libraries actually loads: a file that is present but has
    /// a missing dependency or the wrong architecture answers no, as it would to tray-icon. The
    /// loaded library stays in memory: tray-icon opens it again right after.
    pub fn appindicator_loads() -> bool {
        APPINDICATOR_LIBRARIES.iter().any(|name| {
            // SAFETY: `name` is a NUL-terminated string; a null handle means "not loadable".
            !unsafe { dlopen(name.as_ptr(), DLOPEN_FLAGS) }.is_null()
        })
    }

    /// The shared session bus connection, or null when there is none. The only call without a
    /// timeout: GIO may try `$XDG_RUNTIME_DIR/bus` and X11 autolaunch, so it is made once, in
    /// setup, and the connection is reused.
    ///
    /// There is no shortcut on `DBUS_SESSION_BUS_ADDRESS`: when it is missing GIO tries
    /// `$XDG_RUNTIME_DIR/bus` before X11 autolaunch, so a missing variable does not mean "no
    /// bus" and checking it would give false negatives (autostart, launchers that drop it).
    pub fn session_bus() -> *mut c_void {
        // SAFETY: a plain GIO call; the returned reference (or null) is owned by the caller.
        unsafe { g_bus_get_sync(G_BUS_TYPE_SESSION, ptr::null_mut(), ptr::null_mut()) }
    }

    /// `Properties.Get("org.kde.StatusNotifierWatcher", "IsStatusNotifierHostRegistered")` on
    /// `bus`. A watcher with no registered host owns the name but shows nothing, and
    /// with no watcher the call fails, so one call answers both questions. Every error (closed
    /// connection, 200 ms timeout, unexpected reply) means "no": the close quits.
    ///
    /// `bus` must be a live `GDBusConnection` reference, as returned by [`session_bus`].
    pub fn status_notifier_host_registered(bus: *mut c_void) -> bool {
        // SAFETY: plain GIO calls with valid NUL-terminated strings. `g_variant_new_tuple`
        // sinks the floating strings, `g_dbus_connection_call_sync` sinks the floating tuple,
        // and every returned reference is released exactly once. GIO checks the reply against
        // the `(v)` type before returning it; the boxed value is checked to be a boolean before
        // it is read. `bus` is borrowed: the caller keeps its reference.
        unsafe {
            let arguments = [
                g_variant_new_string(c"org.kde.StatusNotifierWatcher".as_ptr()),
                g_variant_new_string(c"IsStatusNotifierHostRegistered".as_ptr()),
            ];
            let parameters = g_variant_new_tuple(arguments.as_ptr(), arguments.len());
            let reply = g_dbus_connection_call_sync(
                bus,
                c"org.kde.StatusNotifierWatcher".as_ptr(),
                c"/StatusNotifierWatcher".as_ptr(),
                c"org.freedesktop.DBus.Properties".as_ptr(),
                c"Get".as_ptr(),
                parameters,
                c"(v)".as_ptr().cast(),
                G_DBUS_CALL_FLAGS_NONE,
                DBUS_TIMEOUT_MS,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if reply.is_null() {
                return false;
            }
            let boxed = g_variant_get_child_value(reply, 0);
            let value = g_variant_get_variant(boxed);
            let present = g_variant_is_of_type(value, c"b".as_ptr().cast()) != 0
                && g_variant_get_boolean(value) != 0;
            g_variant_unref(value);
            g_variant_unref(boxed);
            g_variant_unref(reply);
            present
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_main_window_hides_and_only_with_a_tray_icon() {
        assert!(hide_on_close(MAIN_WINDOW, true));
        // No visible tray icon: close, or the window could not be reopened.
        assert!(!hide_on_close(MAIN_WINDOW, false));
        // Any other window closes normally.
        assert!(!hide_on_close("overlay", true));
        assert!(!hide_on_close("overlay", false));
        assert!(!hide_on_close("", true));
    }

    #[test]
    fn the_watchdog_restores_only_a_hidden_window_without_a_tray_icon() {
        assert!(restore_hidden_window(false, || false));
        assert!(!restore_hidden_window(false, || true));
        // A visible window is left alone, and the tray is not even asked.
        assert!(!restore_hidden_window(true, || panic!("tray asked")));
    }

    #[test]
    fn a_tray_icon_that_was_not_created_is_never_available() {
        let status = TrayStatus::default();
        assert!(!status.created());
        // Must answer false without asking the session bus.
        assert!(!status.available());
        status.set_created(true);
        assert!(status.created());
    }

    #[test]
    fn the_status_uses_the_documented_shape() {
        let value = serde_json::to_value(AppStatus {
            tray_available: true,
        })
        .unwrap();
        assert_eq!(value, serde_json::json!({ "tray_available": true }));
    }

    /// Calls the real loader and the real session bus: it must answer without crashing whatever
    /// the machine has (CI runners have neither a session bus nor a tray).
    #[test]
    fn the_linux_probes_answer_without_crashing() {
        let _ = linux::appindicator_loads();
        let bus = linux::session_bus();
        if !bus.is_null() {
            let _ = linux::status_notifier_host_registered(bus);
        }
    }

    #[test]
    fn a_created_icon_without_a_session_bus_is_not_available() {
        let status = TrayStatus::default();
        status.set_created(true);
        // `bus` is still null: answers false without any bus call.
        assert!(!status.available());
    }
}
