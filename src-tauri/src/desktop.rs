//! Two calls to the desktop over the session bus: a notification
//! (`org.freedesktop.Notifications.Notify`) and "show this file in the file manager"
//! (`org.freedesktop.FileManager1.ShowItems`).
//!
//! Same approach as the tray (`tray.rs`): GIO/GLib through `extern "C"`, libraries GTK already
//! loads, no extra crate. Every call is blocking and bounded by a D-Bus timeout, so callers run
//! it off the main thread. Every failure (no session bus, no service, timeout) is `false`, never
//! an error: the caller decides what that means.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

const G_BUS_TYPE_SESSION: c_int = 2;
const G_DBUS_CALL_FLAGS_NONE: c_int = 0;
/// A notification server answers at once, unless D-Bus has to start it first.
const NOTIFY_TIMEOUT_MS: c_int = 2_000;
/// The file manager may have to start before it answers.
const SHOW_ITEMS_TIMEOUT_MS: c_int = 10_000;
/// `expire_timeout` of -1: the notification server decides how long it stays.
const DEFAULT_EXPIRY: i32 = -1;

unsafe extern "C" {
    fn g_bus_get_sync(
        bus_type: c_int,
        cancellable: *mut c_void,
        error: *mut *mut c_void,
    ) -> *mut c_void;
    fn g_object_unref(object: *mut c_void);
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
    fn g_variant_new_uint32(value: u32) -> *mut c_void;
    fn g_variant_new_int32(value: i32) -> *mut c_void;
    fn g_variant_new_array(
        child_type: *const c_void,
        children: *const *mut c_void,
        n_children: usize,
    ) -> *mut c_void;
    fn g_variant_new_tuple(children: *const *mut c_void, n_children: usize) -> *mut c_void;
    fn g_variant_ref_sink(value: *mut c_void) -> *mut c_void;
    fn g_variant_unref(value: *mut c_void);
    fn g_filename_to_uri(
        filename: *const c_char,
        hostname: *const c_char,
        error: *mut *mut c_void,
    ) -> *mut c_char;
    fn g_free(memory: *mut c_void);
}

/// Shows a desktop notification. `false` when nothing showed it (no session bus, no
/// notification server, timeout).
pub fn notify(app_name: &str, summary: &str, body: &str) -> bool {
    let (Some(app_name), Some(summary), Some(body)) =
        (c_string(app_name), c_string(summary), c_string(body))
    else {
        return false;
    };
    // SAFETY: every string is NUL-terminated and outlives the calls. The children are floating
    // references: `g_variant_new_tuple` sinks them and `call` sinks the tuple. The empty arrays
    // get their element type from `child_type`, a valid GVariant type string.
    unsafe {
        let children = [
            g_variant_new_string(app_name.as_ptr()),
            g_variant_new_uint32(0),
            g_variant_new_string(c"".as_ptr()),
            g_variant_new_string(summary.as_ptr()),
            g_variant_new_string(body.as_ptr()),
            g_variant_new_array(c"s".as_ptr().cast(), ptr::null(), 0),
            g_variant_new_array(c"{sv}".as_ptr().cast(), ptr::null(), 0),
            g_variant_new_int32(DEFAULT_EXPIRY),
        ];
        let parameters = g_variant_new_tuple(children.as_ptr(), children.len());
        call(
            c"org.freedesktop.Notifications",
            c"/org/freedesktop/Notifications",
            c"org.freedesktop.Notifications",
            c"Notify",
            parameters,
            c"(u)",
            NOTIFY_TIMEOUT_MS,
        )
    }
}

/// Asks the file manager to open the folder of `path` with the file selected. `false` when no
/// file manager implements `org.freedesktop.FileManager1` or it did not answer.
pub fn show_items(path: &Path) -> bool {
    let Some(uri) = file_uri(path).and_then(|uri| CString::new(uri).ok()) else {
        return false;
    };
    // SAFETY: as in `notify`; `uri` outlives the calls and the one-element array sinks its
    // floating child.
    unsafe {
        let uris = [g_variant_new_string(uri.as_ptr())];
        let children = [
            g_variant_new_array(c"s".as_ptr().cast(), uris.as_ptr(), uris.len()),
            g_variant_new_string(c"".as_ptr()),
        ];
        let parameters = g_variant_new_tuple(children.as_ptr(), children.len());
        call(
            c"org.freedesktop.FileManager1",
            c"/org/freedesktop/FileManager1",
            c"org.freedesktop.FileManager1",
            c"ShowItems",
            parameters,
            c"()",
            SHOW_ITEMS_TIMEOUT_MS,
        )
    }
}

/// The `file://` URI of an absolute path, escaped by GLib. `None` for a relative path.
pub fn file_uri(path: &Path) -> Option<String> {
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is NUL-terminated; the result is either null or a string GLib allocated,
    // which is copied and then freed exactly once.
    unsafe {
        let uri = g_filename_to_uri(path.as_ptr(), ptr::null(), ptr::null_mut());
        if uri.is_null() {
            return None;
        }
        let owned = CStr::from_ptr(uri).to_string_lossy().into_owned();
        g_free(uri.cast());
        Some(owned)
    }
}

fn c_string(text: &str) -> Option<CString> {
    CString::new(text).ok()
}

/// One method call on the session bus, `true` when it answered with `reply_type`.
///
/// # Safety
///
/// `parameters` must be a new (floating) GVariant tuple; it is consumed even when there is no
/// bus.
unsafe fn call(
    bus_name: &CStr,
    object_path: &CStr,
    interface: &CStr,
    method: &CStr,
    parameters: *mut c_void,
    reply_type: &CStr,
    timeout_ms: c_int,
) -> bool {
    // SAFETY: plain GIO calls with NUL-terminated strings. The connection is the shared session
    // bus: our reference is released once, after the call. `call_sync` sinks `parameters`; when
    // there is no bus they are sunk and released here instead. The reply, if any, is released.
    unsafe {
        let bus = g_bus_get_sync(G_BUS_TYPE_SESSION, ptr::null_mut(), ptr::null_mut());
        if bus.is_null() {
            g_variant_unref(g_variant_ref_sink(parameters));
            return false;
        }
        let reply = g_dbus_connection_call_sync(
            bus,
            bus_name.as_ptr(),
            object_path.as_ptr(),
            interface.as_ptr(),
            method.as_ptr(),
            parameters,
            reply_type.as_ptr().cast(),
            G_DBUS_CALL_FLAGS_NONE,
            timeout_ms,
            ptr::null_mut(),
            ptr::null_mut(),
        );
        g_object_unref(bus);
        if reply.is_null() {
            return false;
        }
        g_variant_unref(reply);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `notify` and `show_items` are not called here: on a developer's desktop they would show
    // a real notification or open a file manager window.

    #[test]
    fn file_uris_are_escaped_by_glib() {
        assert_eq!(
            file_uri(Path::new("/redline-test/Replays/Replay 1 #2.mkv")).as_deref(),
            Some("file:///redline-test/Replays/Replay%201%20%232.mkv")
        );
        assert_eq!(file_uri(Path::new("relative.mkv")), None);
    }

    #[test]
    fn strings_with_a_nul_are_refused_before_any_call() {
        assert!(!notify("Redline", "Replay saved", "a\0b"));
    }
}
