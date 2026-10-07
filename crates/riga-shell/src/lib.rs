//! Startup helpers shared by RIGA Tauri desktop shells.
//!
//! These belong to the **native shell**, not to `riga-kernel`, `riga-server`, or
//! the web UI: they set process-global environment that GTK and WebKitGTK read
//! while initializing, so they must run before the Tauri runtime starts. Keeping
//! them here means each shell calls one function and the app code stays clean.

/// Prepare the process for GTK/WebKitGTK on Linux before the toolkit starts.
///
/// Some Wayland sessions expose no cursor theme to GTK, and the GTK runtime then
/// aborts while creating the first window — the process logs
/// `Gdk:ERROR … _gdk_wayland_display_get_scaled_cursor_theme: assertion failed`
/// and dies before the webview appears. Set conservative cursor defaults, and
/// prefer XWayland when a `DISPLAY` is also available, because that path is
/// unaffected.
///
/// Call this once, first thing, in the desktop shell's `main`.
#[cfg(target_os = "linux")]
pub fn prepare_linux_display() {
    if std::env::var_os("XCURSOR_THEME").is_none() {
        std::env::set_var("XCURSOR_THEME", "Adwaita");
    }
    if std::env::var_os("XCURSOR_SIZE").is_none() {
        std::env::set_var("XCURSOR_SIZE", "24");
    }
    if std::env::var_os("GDK_BACKEND").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("DISPLAY").is_some()
    {
        std::env::set_var("GDK_BACKEND", "x11");
    }
}

/// No-op on platforms whose toolkit does not need it, so callers can invoke it
/// unconditionally.
#[cfg(not(target_os = "linux"))]
pub fn prepare_linux_display() {}
