//! Per-session git worktrees.
//!
//! A run edits its own worktree on a `riga/<session>` branch, forked from the
//! base repository's `HEAD`. That gives each session a private working
//! directory: two sessions can create an app called `web` without colliding,
//! and the base checkout is never touched. Reviewing a session is a normal
//! `git diff`/merge of its branch.
//!
//! Worktrees live under `<base>/.riga/worktrees/`, which is gitignored and
//! pruned from the workspace tools, so the main checkout stays clean and no
//! tool descends into another session's tree.

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Directory, relative to the base repository, that holds session worktrees.
pub const WORKTREE_DIR: &str = ".riga/worktrees";

/// Reduce a client-supplied session id to a safe path and branch component.
pub fn sanitize_session(session_id: &str) -> String {
    let cleaned: String = session_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "session".to_owned()
    } else {
        cleaned
    }
}

/// The worktree directory for a session, whether or not it exists yet.
pub fn worktree_path(base: &Path, session_id: &str) -> PathBuf {
    base.join(WORKTREE_DIR).join(sanitize_session(session_id))
}

/// The branch a session's worktree is checked out on.
pub fn branch_name(session_id: &str) -> String {
    format!("riga/{}", sanitize_session(session_id))
}

/// Ensure the session's worktree exists and return its path.
///
/// Forks from the base repository's `HEAD` at first call and is idempotent
/// afterwards, so every run in a session edits the same tree. A workspace that
/// is not a git checkout is used directly: isolation is unavailable, but the
/// agent still works rather than failing outright.
pub async fn ensure_worktree(base: &Path, session_id: &str) -> Result<PathBuf, String> {
    let path = worktree_path(base, session_id);
    // The `.git` *file* is what a linked worktree has; the base repo has a
    // `.git` directory. Checking for either distinguishes "already created"
    // from "not yet".
    if path.join(".git").exists() {
        return Ok(path);
    }
    if !base.join(".git").exists() {
        tracing::warn!(
            workspace = %base.display(),
            "workspace is not a git repository; running without worktree isolation"
        );
        return Ok(base.to_path_buf());
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("could not create the worktree directory: {error}"))?;
    }
    let branch = branch_name(session_id);
    // A fresh branch; if it already exists (a removed-and-recreated worktree),
    // attach to it instead.
    if run_git(
        base,
        &["worktree", "add", "-b", &branch, &path.to_string_lossy()],
    )
    .await
    {
        return Ok(path);
    }
    if run_git(base, &["worktree", "add", &path.to_string_lossy(), &branch]).await {
        return Ok(path);
    }
    Err(format!(
        "git worktree add failed for branch `{branch}`; run `git worktree list` in the workspace"
    ))
}

/// Run a git command in `base`, returning whether it succeeded. stderr is
/// surfaced through `tracing` so a failure is diagnosable without failing the
/// caller, which retries or falls back.
async fn run_git(base: &Path, args: &[&str]) -> bool {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            tracing::warn!(
                args = ?args,
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "git command failed"
            );
            false
        }
        Err(error) => {
            tracing::warn!(?error, "git could not be executed");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{branch_name, ensure_worktree, sanitize_session, worktree_path};

    #[test]
    fn session_ids_are_sanitized_for_paths_and_branches() {
        assert_eq!(sanitize_session("session-1"), "session-1");
        assert_eq!(sanitize_session("../../etc/passwd"), "------etc-passwd");
        assert_eq!(
            sanitize_session("550e8400-e29b-41d4-a716-446655440000"),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(sanitize_session(""), "session");
        assert_eq!(branch_name("session 1"), "riga/session-1");
    }

    #[test]
    fn worktree_path_is_under_the_ignored_directory() {
        let path = worktree_path(std::path::Path::new("/repo"), "abc");
        assert!(path.ends_with(".riga/worktrees/abc"), "{}", path.display());
    }

    #[tokio::test]
    async fn worktrees_fork_from_head_and_are_isolated_per_session() {
        // Real git, in a throwaway repo.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(base)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(base.join("seed.txt"), "from head").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "seed"]);

        let first = ensure_worktree(base, "session-1").await.unwrap();
        // A session worktree is a checkout of HEAD.
        assert_eq!(
            std::fs::read_to_string(first.join("seed.txt")).unwrap(),
            "from head"
        );
        assert_eq!(first, worktree_path(base, "session-1"));
        // Idempotent: a second call returns the same tree rather than failing on
        // the existing branch.
        assert_eq!(ensure_worktree(base, "session-1").await.unwrap(), first);

        // A different session gets a different directory, so two runs can
        // create the same path without colliding.
        let second = ensure_worktree(base, "session-2").await.unwrap();
        assert_ne!(first, second);
        std::fs::write(first.join("app.txt"), "one").unwrap();
        std::fs::write(second.join("app.txt"), "two").unwrap();
        assert_eq!(
            std::fs::read_to_string(first.join("app.txt")).unwrap(),
            "one"
        );
        assert_eq!(
            std::fs::read_to_string(second.join("app.txt")).unwrap(),
            "two"
        );

        // The base checkout is untouched by either session.
        assert!(!base.join("app.txt").exists());
    }

    #[tokio::test]
    async fn a_non_git_workspace_falls_back_to_itself() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        assert_eq!(ensure_worktree(base, "session-1").await.unwrap(), base);
    }
}
