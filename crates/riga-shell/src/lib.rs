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

/// Default per-user data directory, matching `riga-server`'s store location:
/// `RIGA_DATA_DIR`, else `$HOME/.local/share/riga`, else `./.riga-data`.
pub fn default_data_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("RIGA_DATA_DIR") {
        return std::path::PathBuf::from(dir);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return std::path::PathBuf::from(home).join(".local/share/riga");
    }
    std::path::PathBuf::from(".riga-data")
}

/// Point `RIGA_WORKSPACE_ROOT` at a deliberate, git-initialized workspace.
///
/// A packaged app inherits its working directory from the launcher (often
/// `$HOME`), and `tauri dev` uses `src-tauri`; neither is a repository, so
/// `riga-server`'s per-session worktrees never engage and uploads land in the
/// watched source tree. Give the agent its own workspace instead: created and
/// `git init`-ed (with an initial commit, so `git worktree add` has a `HEAD` to
/// fork from), it keeps worktrees and attachments out of the app and the dev
/// watcher.
///
/// An explicit `RIGA_WORKSPACE_ROOT` is respected, so a host can point at a real
/// project instead.
pub fn ensure_workspace(workspace: &std::path::Path) -> std::io::Result<()> {
    if std::env::var_os("RIGA_WORKSPACE_ROOT").is_some() {
        return Ok(());
    }
    std::fs::create_dir_all(workspace)?;
    if !workspace.join(".git").exists() {
        init_workspace_repo(workspace)?;
    }
    std::env::set_var("RIGA_WORKSPACE_ROOT", workspace);
    Ok(())
}

/// `git init` plus an empty initial commit, so worktrees can fork from `HEAD`.
fn init_workspace_repo(workspace: &std::path::Path) -> std::io::Result<()> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(args)
            .status()
    };
    if !git(&["init", "-q"])?.success() {
        return Err(std::io::Error::other("git init failed"));
    }
    // A local identity lets the initial commit succeed without global config.
    let _ = git(&["config", "user.email", "riga@localhost"]);
    let _ = git(&["config", "user.name", "RIGA"]);
    if !git(&[
        "commit",
        "--allow-empty",
        "-q",
        "-m",
        "Initialize RIGA workspace",
    ])?
    .success()
    {
        return Err(std::io::Error::other("initial workspace commit failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_workspace;

    #[test]
    fn ensure_workspace_initializes_a_repo_and_sets_the_env() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        // Clear any ambient override so the helper does not short-circuit.
        std::env::remove_var("RIGA_WORKSPACE_ROOT");
        ensure_workspace(&workspace).unwrap();
        // A real repo, so per-session worktrees can fork from HEAD.
        assert!(workspace.join(".git").exists());
        assert_eq!(
            std::env::var_os("RIGA_WORKSPACE_ROOT"),
            Some(workspace.clone().into_os_string())
        );
        std::env::remove_var("RIGA_WORKSPACE_ROOT");
    }
}
