//! Workspace browsing and git operations for the desktop/browser explorer.
//!
//! Every path is resolved relative to a session's worktree and is rejected if it
//! tries to escape it. Directory listing is deliberately one level at a time so
//! the UI can expand lazily instead of walking a whole `node_modules` up front.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;

/// Cap on the bytes returned for one file preview.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Cap on the entries returned for one directory listing.
const MAX_DIR_ENTRIES: usize = 2_000;

#[derive(Debug, Serialize)]
pub struct WorkspaceEntry {
    pub name: String,
    /// Forward-slash path relative to the worktree root.
    pub path: String,
    /// `dir` or `file`.
    pub kind: &'static str,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceFile {
    pub path: String,
    pub text: String,
    pub size: u64,
    pub truncated: bool,
    pub binary: bool,
}

#[derive(Debug, Serialize)]
pub struct GitChange {
    pub path: String,
    /// `modified`, `added`, `deleted`, `renamed`, `untracked`, or `conflicted`.
    pub status: String,
    pub staged: bool,
    pub untracked: bool,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceDiff {
    pub path: String,
    pub status: String,
    pub diff: String,
    pub binary: bool,
}

#[derive(Debug, Serialize)]
pub struct GitCommitResult {
    pub committed: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct GitPushResult {
    pub pushed: bool,
    pub message: String,
}

/// Resolve a client path inside the workspace, rejecting anything that escapes.
pub fn safe_relative(relative: &str) -> Result<PathBuf, String> {
    if relative.contains('\0') {
        return Err("path contains a null byte".to_owned());
    }
    let mut out = PathBuf::new();
    for part in relative.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return Err("path must stay inside the workspace".to_owned()),
            other => out.push(other),
        }
    }
    Ok(out)
}

fn to_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// List one directory level, directories first and each group name-sorted.
pub fn list_dir(root: &Path, relative: &str) -> Result<Vec<WorkspaceEntry>, String> {
    let relative = safe_relative(relative)?;
    let dir = root.join(&relative);
    let entries = std::fs::read_dir(&dir)
        .map_err(|error| format!("could not read {}: {error}", dir.display()))?;
    let mut collected: Vec<WorkspaceEntry> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // The worktree's `.git` is an implementation detail, not a browsable file.
        if name == ".git" {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let kind = if file_type.is_dir() { "dir" } else { "file" };
        collected.push(WorkspaceEntry {
            name: name.clone(),
            path: to_slash(&relative.join(&name)),
            kind,
        });
        if collected.len() >= MAX_DIR_ENTRIES {
            break;
        }
    }
    collected.sort_by(|left, right| {
        let left_dir = left.kind == "dir";
        let right_dir = right.kind == "dir";
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(collected)
}

/// Read a file's text, capped, with a binary check so the preview never mojibakes.
pub fn read_file(root: &Path, relative: &str) -> Result<WorkspaceFile, String> {
    let relative = safe_relative(relative)?;
    if relative.as_os_str().is_empty() {
        return Err("path is required".to_owned());
    }
    let path = root.join(&relative);
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("could not stat {}: {error}", path.display()))?;
    if metadata.is_dir() {
        return Err("that path is a directory".to_owned());
    }
    let size = metadata.len();
    let bytes =
        std::fs::read(&path).map_err(|error| format!("could not read the file: {error}"))?;
    let truncated = size > MAX_FILE_BYTES;
    let kept = &bytes[..bytes.len().min(MAX_FILE_BYTES as usize)];
    let binary = kept.iter().take(8_000).any(|byte| *byte == 0);
    let text = if binary {
        String::new()
    } else {
        String::from_utf8_lossy(kept).into_owned()
    };
    Ok(WorkspaceFile {
        path: to_slash(&relative),
        text,
        size,
        truncated,
        binary,
    })
}

async fn run_git_output(root: &Path, args: &[&str]) -> Result<(bool, String, String), String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        // Never block on an interactive credential prompt: a commit/push from the
        // GUI must fail fast with a readable error instead of hanging the panel.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| format!("git could not be executed: {error}"))?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

fn change_status(code: &str) -> String {
    // Prefer the worktree column, then the index column.
    let letter = code
        .chars()
        .rev()
        .find(|character| *character != ' ' && *character != '?')
        .or_else(|| {
            code.chars()
                .find(|character| *character != ' ' && *character != '?')
        });
    match letter {
        Some('M') => "modified",
        Some('A') => "added",
        Some('D') => "deleted",
        Some('R') => "renamed",
        Some('C') => "renamed",
        Some('U') => "conflicted",
        _ if code.starts_with("??") => "untracked",
        _ => "modified",
    }
    .to_owned()
}

/// Parse `git status --porcelain=v1 -z` into display changes.
pub fn parse_status(raw: &str) -> Vec<GitChange> {
    let mut changes = Vec::new();
    let mut parts = raw.split('\0');
    while let Some(token) = parts.next() {
        if token.len() < 3 {
            continue;
        }
        let code = &token[..2];
        let path = token[3..].to_owned();
        // A rename/copy entry is followed by the original path.
        if code.starts_with('R') || code.starts_with('C') {
            let _ = parts.next();
        }
        let untracked = code == "??";
        changes.push(GitChange {
            status: change_status(code),
            staged: !untracked && !code.starts_with(' ') && !code.starts_with('?'),
            untracked,
            path,
        });
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    changes
}

pub async fn git_status(root: &Path) -> Result<Vec<GitChange>, String> {
    let (ok, stdout, stderr) = run_git_output(root, &["status", "--porcelain=v1", "-z"]).await?;
    if !ok {
        return Err(format!("git status failed: {}", stderr.trim()));
    }
    Ok(parse_status(&stdout))
}

fn synthesized_added_diff(path: &str, text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut diff = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in &lines {
        diff.push('+');
        diff.push_str(line.strip_suffix('\n').unwrap_or(line));
        diff.push('\n');
    }
    diff
}

pub async fn git_diff(root: &Path, relative: &str) -> Result<WorkspaceDiff, String> {
    let relative = safe_relative(relative)?;
    if relative.as_os_str().is_empty() {
        return Err("path is required".to_owned());
    }
    let display = to_slash(&relative);
    let statuses = git_status(root).await?;
    let change = statuses.iter().find(|change| change.path == display);
    let status = change
        .map(|change| change.status.clone())
        .unwrap_or_else(|| "modified".to_owned());
    if change.is_some_and(|change| change.untracked) {
        // `git diff` prints nothing for an untracked file, so show it as an add.
        let file = read_file(root, &display)?;
        if file.binary {
            return Ok(WorkspaceDiff {
                path: display,
                status,
                diff: String::new(),
                binary: true,
            });
        }
        return Ok(WorkspaceDiff {
            path: display.clone(),
            status,
            diff: synthesized_added_diff(&display, &file.text),
            binary: false,
        });
    }
    let (ok, stdout, stderr) = run_git_output(root, &["diff", "HEAD", "--", &display]).await?;
    if !ok {
        return Err(format!("git diff failed: {}", stderr.trim()));
    }
    let binary = stdout.contains("Binary files");
    Ok(WorkspaceDiff {
        path: display,
        status,
        diff: stdout,
        binary,
    })
}

/// One commit from `git log`.
#[derive(Debug, Serialize)]
pub struct GitCommit {
    pub id: String,
    pub short: String,
    pub message: String,
    pub author: String,
    pub timestamp: i64,
}

/// Read the most recent commits on the worktree's branch.
pub async fn git_log(root: &Path, limit: usize) -> Result<Vec<GitCommit>, String> {
    let limit = limit.clamp(1, 200).to_string();
    let (ok, stdout, stderr) = run_git_output(
        root,
        &[
            "log",
            "--max-count",
            &limit,
            "--pretty=format:%H%x1f%h%x1f%an%x1f%at%x1f%s",
        ],
    )
    .await?;
    if !ok {
        return Err(format!("git log failed: {}", stderr.trim()));
    }
    Ok(stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut parts = line.split('\u{1f}');
            GitCommit {
                id: parts.next().unwrap_or_default().to_owned(),
                short: parts.next().unwrap_or_default().to_owned(),
                author: parts.next().unwrap_or_default().to_owned(),
                timestamp: parts
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
                message: parts.next().unwrap_or_default().to_owned(),
            }
        })
        .collect())
}

/// The patch one commit introduced.
pub async fn git_commit_diff(root: &Path, commit: &str) -> Result<WorkspaceDiff, String> {
    let commit = commit.trim();
    // A commit is a hash or a ref; reject anything that could read as an option.
    if commit.is_empty() || commit.starts_with('-') || commit.contains('\0') {
        return Err("invalid commit id".to_owned());
    }
    let (ok, stdout, stderr) = run_git_output(
        root,
        &["show", "--no-color", "--format=", "--patch", commit],
    )
    .await?;
    if !ok {
        return Err(format!("git show failed: {}", stderr.trim()));
    }
    Ok(WorkspaceDiff {
        path: commit.to_owned(),
        status: "commit".to_owned(),
        binary: stdout.contains("Binary files"),
        diff: stdout,
    })
}

async fn git_identity(root: &Path) -> Option<(String, String)> {
    let (_, name, _) = run_git_output(root, &["config", "user.name"]).await.ok()?;
    let (_, email, _) = run_git_output(root, &["config", "user.email"]).await.ok()?;
    let name = name.trim().to_owned();
    let email = email.trim().to_owned();
    if name.is_empty() || email.is_empty() {
        None
    } else {
        Some((name, email))
    }
}

pub async fn git_commit(root: &Path, message: &str) -> Result<GitCommitResult, String> {
    if message.trim().is_empty() {
        return Err("a commit message is required".to_owned());
    }
    let (added, _, add_err) = run_git_output(root, &["add", "-A"]).await?;
    if !added {
        return Err(format!("git add failed: {}", add_err.trim()));
    }
    // Exit 0 from `diff --cached --quiet` means nothing is staged.
    let (clean, _, _) = run_git_output(root, &["diff", "--cached", "--quiet"]).await?;
    if clean {
        return Ok(GitCommitResult {
            committed: false,
            message: "Nothing to commit; the worktree is clean.".to_owned(),
        });
    }
    // A fresh worktree has no author identity, so supply a local fallback rather
    // than failing the whole commit.
    let mut args: Vec<String> = Vec::new();
    if git_identity(root).await.is_none() {
        args.extend([
            "-c".to_owned(),
            "user.name=RIGA".to_owned(),
            "-c".to_owned(),
            "user.email=riga@localhost".to_owned(),
        ]);
    }
    args.push("commit".to_owned());
    args.push("-m".to_owned());
    args.push(message.trim().to_owned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (committed, commit_out, commit_err) = run_git_output(root, &arg_refs).await?;
    if !committed {
        return Err(format!("git commit failed: {}", commit_err.trim()));
    }
    Ok(GitCommitResult {
        committed: true,
        message: commit_out
            .lines()
            .next()
            .unwrap_or("Committed.")
            .trim()
            .to_owned(),
    })
}

/// Push the current branch, publishing it on `origin` when it has no upstream.
pub async fn git_push(root: &Path) -> Result<GitPushResult, String> {
    let (pushed, _, push_err) = run_git_output(root, &["push"]).await?;
    if pushed {
        return Ok(GitPushResult {
            pushed: true,
            message: "Pushed.".to_owned(),
        });
    }
    // No upstream yet: publish the session branch.
    let (published, _, publish_err) =
        run_git_output(root, &["push", "-u", "origin", "HEAD"]).await?;
    if published {
        return Ok(GitPushResult {
            pushed: true,
            message: "Pushed and set the upstream.".to_owned(),
        });
    }
    let detail = if push_err.trim().is_empty() {
        publish_err
    } else {
        push_err
    };
    Ok(GitPushResult {
        pushed: false,
        message: format!("git push failed: {}", detail.trim()),
    })
}

#[cfg(test)]
mod tests {
    use super::{list_dir, parse_status, read_file, safe_relative};

    #[test]
    fn relative_paths_cannot_escape_the_workspace() {
        assert!(safe_relative("src/app.tsx").is_ok());
        assert!(safe_relative("/etc/passwd").is_ok());
        assert_eq!(
            safe_relative("/etc/passwd").unwrap().to_string_lossy(),
            "etc/passwd"
        );
        assert!(safe_relative("../secrets").is_err());
        assert!(safe_relative("a/../../b").is_err());
    }

    #[test]
    fn status_parser_reads_index_and_worktree_columns() {
        let raw = " M src/app.tsx\0?? new.txt\0A  staged.ts\0R  new-name.ts\0old-name.ts\0";
        let changes = parse_status(raw);
        assert!(
            changes
                .iter()
                .any(|change| change.path == "src/app.tsx" && !change.staged)
        );
        assert!(
            changes
                .iter()
                .any(|change| change.path == "new.txt" && change.untracked)
        );
        assert!(
            changes
                .iter()
                .any(|change| change.path == "staged.ts" && change.staged)
        );
        assert!(
            changes
                .iter()
                .any(|change| change.path == "new-name.ts" && change.status == "renamed")
        );
        assert!(!changes.iter().any(|change| change.path == "old-name.ts"));
    }

    #[test]
    fn listing_and_reading_one_level() {
        let dir = std::env::temp_dir().join(format!("riga-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("readme.md"), "# hi\n").unwrap();

        let entries = list_dir(&dir, "").unwrap();
        assert_eq!(entries[0].name, "src");
        assert_eq!(entries[0].kind, "dir");
        assert_eq!(entries[1].name, "readme.md");
        let nested = list_dir(&dir, "src").unwrap();
        assert_eq!(nested[0].path, "src/main.rs");

        let file = read_file(&dir, "readme.md").unwrap();
        assert_eq!(file.text, "# hi\n");
        assert!(!file.binary);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
