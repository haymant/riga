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
    // The workspace starts empty, so the bundled `skills/` never reach it and
    // the composer's insert menu lists none. Seed them (a user's edited copy
    // wins over the bundled one).
    seed_bundled_skills(workspace, bundled_skills_source().as_deref());
    std::env::set_var("RIGA_WORKSPACE_ROOT", workspace);
    Ok(())
}

/// Where the shell's bundled skills live, if any.
///
/// `RIGA_SKILLS_DIR` overrides; otherwise the repository `skills/` next to this
/// crate at build time. That is present under `tauri dev` and absent in a
/// packaged app that does not ship the source tree, in which case there is
/// simply nothing to seed.
fn bundled_skills_source() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("RIGA_SKILLS_DIR") {
        return Some(std::path::PathBuf::from(dir));
    }
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    repo.is_dir().then_some(repo)
}

/// Copy each bundled `<name>/SKILL.md` into `<workspace>/skills/<name>/SKILL.md`.
///
/// A skill the user has edited is left untouched; an unedited copy is refreshed
/// when the bundled version changes. A plain "copy only if missing" would never
/// ship a skill update to an existing workspace, so a manifest at
/// `<workspace>/.riga/skills-seed` records the bundled hash last written per
/// skill — that is how an unedited copy is told from an edited one. A copy with
/// no manifest entry (a first run of this seeder, or a hand-placed file) is
/// treated as unedited, so a shipped update reaches an existing workspace.
fn seed_bundled_skills(workspace: &std::path::Path, source: Option<&std::path::Path>) {
    let Some(source) = source else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(source) else {
        return;
    };
    let dest = workspace.join("skills");
    let manifest_path = workspace.join(".riga/skills-seed");
    let mut manifest: std::collections::HashMap<String, String> =
        std::fs::read_to_string(&manifest_path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(name, hash)| (name.to_owned(), hash.to_owned()))
            .collect();
    for entry in entries.flatten() {
        let from = entry.path();
        if !from.join("SKILL.md").is_file() {
            continue;
        }
        let Ok(bundled) = std::fs::read_to_string(from.join("SKILL.md")) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let to = dest.join(entry.file_name()).join("SKILL.md");
        match std::fs::read_to_string(&to) {
            Ok(existing) => {
                // Refresh a copy we wrote and the user has not since edited. With
                // no manifest entry (first run of this seeder, or a hand-placed
                // copy) the bundled skill is authoritative, so it refreshes too.
                let unedited = manifest
                    .get(&name)
                    .map(|hash| hash == &content_hash(&existing))
                    .unwrap_or(true);
                if unedited && existing != bundled {
                    let _ = std::fs::write(&to, &bundled);
                }
            }
            Err(_) => {
                if std::fs::create_dir_all(dest.join(entry.file_name())).is_ok() {
                    let _ = std::fs::write(&to, &bundled);
                }
            }
        }
        manifest.insert(name, content_hash(&bundled));
    }
    if let Some(parent) = manifest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut names: Vec<&String> = manifest.keys().collect();
    names.sort();
    let text: String = names
        .into_iter()
        .filter_map(|name| manifest.get(name).map(|hash| format!("{name}\t{hash}\n")))
        .collect();
    let _ = std::fs::write(&manifest_path, text);
}

/// Stable FNV-1a hash of a skill's contents, for the seed manifest. Not
/// `DefaultHasher`, whose output is not guaranteed stable across releases — a
/// changed hash would make an unedited copy look edited and stop refreshing.
fn content_hash(content: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
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

    #[test]
    fn ensure_workspace_seeds_and_refreshes_bundled_skills() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled");
        std::fs::create_dir_all(source.join("demo")).unwrap();
        std::fs::write(source.join("demo/SKILL.md"), "bundled").unwrap();

        // A copy with no manifest entry is treated as an old bundled copy and
        // refreshed, so a shipped skill update reaches an existing workspace.
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(workspace.join("skills/demo")).unwrap();
        std::fs::write(workspace.join("skills/demo/SKILL.md"), "stale").unwrap();
        super::seed_bundled_skills(&workspace, Some(&source));
        assert_eq!(
            std::fs::read_to_string(workspace.join("skills/demo/SKILL.md")).unwrap(),
            "bundled"
        );

        // A fresh workspace receives the bundled skill.
        let fresh = dir.path().join("fresh");
        super::seed_bundled_skills(&fresh, Some(&source));
        assert_eq!(
            std::fs::read_to_string(fresh.join("skills/demo/SKILL.md")).unwrap(),
            "bundled"
        );
    }

    #[test]
    fn ensure_workspace_refreshes_an_unedited_skill_but_keeps_an_edited_one() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundled");
        std::fs::create_dir_all(source.join("demo")).unwrap();
        std::fs::write(source.join("demo/SKILL.md"), "v1").unwrap();
        let workspace = dir.path().join("workspace");
        let read = || std::fs::read_to_string(workspace.join("skills/demo/SKILL.md")).unwrap();

        // First seed installs v1 and records its hash.
        super::seed_bundled_skills(&workspace, Some(&source));
        assert_eq!(read(), "v1");

        // The bundled skill changes: an unedited copy is refreshed, which a
        // plain "copy only if missing" would never do.
        std::fs::write(source.join("demo/SKILL.md"), "v2").unwrap();
        super::seed_bundled_skills(&workspace, Some(&source));
        assert_eq!(read(), "v2");

        // A user edit is preserved across the next bundled change.
        std::fs::write(workspace.join("skills/demo/SKILL.md"), "mine").unwrap();
        std::fs::write(source.join("demo/SKILL.md"), "v3").unwrap();
        super::seed_bundled_skills(&workspace, Some(&source));
        assert_eq!(read(), "mine");
    }
}
