use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde::{Deserialize, Serialize};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogItem {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub insert_text: String,
    pub requires_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerSummary {
    pub name: String,
    pub command: String,
    pub tools: Vec<String>,
}

pub fn builtins() -> Vec<CatalogItem> {
    [
        ("read", "Read a UTF-8 file inside the workspace", false),
        (
            "write",
            "Write or replace a UTF-8 file inside the workspace",
            true,
        ),
        (
            "glob",
            "Find files by a workspace-relative glob pattern",
            false,
        ),
        ("grep", "Search text in workspace files", false),
        ("web", "Fetch a public HTTPS page", true),
        (
            "bash",
            "Run a shell command in the workspace; disabled unless explicitly enabled",
            true,
        ),
        ("shell", "Alias for bash", true),
        ("task", "Create or inspect a RIGA task", false),
        ("skill", "Load a repository skill document", false),
    ]
    .into_iter()
    .map(|(id, description, requires_approval)| CatalogItem {
        id: id.into(),
        kind: "builtin".into(),
        description: description.into(),
        insert_text: format!("Use the {id} tool: "),
        requires_approval,
    })
    .collect()
}

pub fn load_skills(root: &Path) -> Vec<SkillSummary> {
    let skills_root = root.join("skills");
    let Ok(entries) = fs::read_dir(&skills_root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path().join("SKILL.md");
            let text = fs::read_to_string(&path).ok()?;
            let description = text
                .lines()
                .find_map(|line| {
                    line.strip_prefix("description:")
                        .map(str::trim)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| {
                    text.lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("Skill")
                        .trim_start_matches('#')
                        .trim()
                        .to_owned()
                });
            Some(SkillSummary {
                name: entry.file_name().to_string_lossy().into_owned(),
                description,
                path: path.display().to_string(),
            })
        })
        .collect()
}

pub fn load_mcp_servers(root: &Path) -> Vec<McpServerSummary> {
    let path = root.join(".mcp.json");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    value
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .map(|servers| {
            servers
                .iter()
                .map(|(name, value)| McpServerSummary {
                    name: name.clone(),
                    command: value
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown")
                        .into(),
                    tools: Vec::new(),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn workspace_root() -> PathBuf {
    std::env::var_os("RIGA_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigaTask {
    pub id: String,
    pub title: String,
    pub status: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn execute_task(input: &serde_json::Value) -> Result<String, String> {
    let store = crate::secure_store::SecureStore::from_env()
        .ok_or("task persistence requires RIGA_TOKEN")?;
    let mut tasks = store.load::<Vec<RigaTask>>("tasks")?.unwrap_or_default();
    let action = input
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("list");
    match action {
        "list" => serde_json::to_string_pretty(&tasks).map_err(|error| error.to_string()),
        "inspect" => {
            let id = input
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("task inspect requires task_id")?;
            let task = tasks
                .iter()
                .find(|task| task.id == id)
                .ok_or_else(|| format!("task not found: {id}"))?;
            serde_json::to_string_pretty(task).map_err(|error| error.to_string())
        }
        "create" => {
            let title = input
                .get("title")
                .and_then(serde_json::Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .ok_or("task create requires title")?;
            let now = unix_timestamp();
            let task = RigaTask {
                id: format!("task-{now}"),
                title: title.to_owned(),
                status: input
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("open")
                    .to_owned(),
                description: input
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                created_at: now.to_string(),
                updated_at: now.to_string(),
            };
            tasks.push(task.clone());
            store.save("tasks", &tasks)?;
            serde_json::to_string_pretty(&task).map_err(|error| error.to_string())
        }
        "update" => {
            let id = input
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("task update requires task_id")?;
            let task = tasks
                .iter_mut()
                .find(|task| task.id == id)
                .ok_or_else(|| format!("task not found: {id}"))?;
            if let Some(title) = input.get("title").and_then(serde_json::Value::as_str) {
                task.title = title.to_owned();
            }
            if let Some(status) = input.get("status").and_then(serde_json::Value::as_str) {
                task.status = status.to_owned();
            }
            if let Some(description) = input.get("description").and_then(serde_json::Value::as_str)
            {
                task.description = description.to_owned();
            }
            task.updated_at = unix_timestamp().to_string();
            let updated = task.clone();
            store.save("tasks", &tasks)?;
            serde_json::to_string_pretty(&updated).map_err(|error| error.to_string())
        }
        _ => Err("task action must be list, inspect, create, or update".into()),
    }
}

fn unix_timestamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn safe_path(root: &Path, requested: &str) -> Result<PathBuf, String> {
    let candidate = root.join(requested);
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canonical_parent = candidate
        .parent()
        .unwrap_or(root)
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    if !canonical_parent.starts_with(&canonical_root) {
        return Err("path escapes the configured workspace".into());
    }
    Ok(candidate)
}

pub async fn execute_read(root: &Path, path: &str) -> Result<String, String> {
    let path = safe_path(root, path)?;
    tokio::fs::read_to_string(path)
        .await
        .map_err(|e| e.to_string())
}

pub async fn execute_write(root: &Path, path: &str, content: &str) -> Result<String, String> {
    let path = safe_path(root, path)?;
    if std::env::var("RIGA_ENABLE_WRITES").ok().as_deref() != Some("1") {
        return Err(
            "workspace writes are disabled; set RIGA_ENABLE_WRITES=1 and require approval".into(),
        );
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|e| e.to_string())?;
    Ok("file written".into())
}

pub async fn execute_bash(root: &Path, command: &str) -> Result<String, String> {
    if std::env::var("RIGA_ENABLE_SHELL").ok().as_deref() != Some("1") {
        return Err("shell execution is disabled; set RIGA_ENABLE_SHELL=1 for an explicitly trusted local server".into());
    }
    let mut child = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut output = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_end(&mut output)
            .await
            .map_err(|e| e.to_string())?;
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output)
        .chars()
        .take(20_000)
        .collect::<String>();
    if status.success() {
        Ok(text)
    } else {
        Err(format!("command failed: {text}"))
    }
}

pub fn execute_glob(root: &Path, pattern: &str) -> Result<String, String> {
    let pattern = pattern.trim_start_matches("**/");
    let matches = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            let relative = entry
                .path()
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            if relative.ends_with(pattern)
                || (pattern.starts_with('*') && relative.ends_with(pattern.trim_start_matches('*')))
            {
                Some(relative)
            } else {
                None
            }
        })
        .take(500)
        .collect::<Vec<_>>();
    Ok(matches.join("\n"))
}

pub async fn execute_grep(root: &Path, query: &str) -> Result<String, String> {
    let mut matches = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .take(2_000)
    {
        if let Ok(text) = tokio::fs::read_to_string(entry.path()).await {
            for (line_number, line) in text.lines().enumerate() {
                if line.contains(query) {
                    let relative = entry
                        .path()
                        .strip_prefix(root)
                        .unwrap_or(entry.path())
                        .display();
                    matches.push(format!("{relative}:{}:{line}", line_number + 1));
                    if matches.len() >= 500 {
                        return Ok(matches.join("\n"));
                    }
                }
            }
        }
    }
    Ok(matches.join("\n"))
}

pub async fn execute_web(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    if parsed.scheme() != "https" {
        return Err("web only permits HTTPS URLs".into());
    }
    reqwest::Client::new()
        .get(parsed)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map(|text| text.chars().take(20_000).collect())
        .map_err(|e| e.to_string())
}

pub fn catalog(root: &Path) -> BTreeMap<String, serde_json::Value> {
    let mut result = BTreeMap::new();
    result.insert("tools".into(), serde_json::to_value(builtins()).unwrap());
    result.insert(
        "skills".into(),
        serde_json::to_value(load_skills(root)).unwrap(),
    );
    result.insert(
        "mcp_servers".into(),
        serde_json::to_value(load_mcp_servers(root)).unwrap(),
    );
    result
}
