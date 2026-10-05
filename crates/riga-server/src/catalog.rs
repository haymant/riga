use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde::{Deserialize, Serialize};
use tokio::{io::AsyncReadExt, process::Command, sync::mpsc};

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
pub struct FileCandidate {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerSummary {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub tools: Vec<String>,
    #[serde(rename = "transport", skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub api_key_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerRecord {
    pub summary: McpServerSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

pub fn builtin_health_stdio() -> McpServerSummary {
    McpServerSummary {
        name: "riga-health-stdio".into(),
        command: "riga-server".into(),
        args: vec!["mcp-health-stdio".into()],
        tools: vec!["health".into()],
        transport: Some("stdio".into()),
        url: None,
        api_key_configured: false,
    }
}

pub fn builtin_health_http() -> McpServerSummary {
    McpServerSummary {
        name: "riga-health-http".into(),
        command: "http-stream".into(),
        args: Vec::new(),
        tools: vec!["health".into()],
        transport: Some("http".into()),
        url: Some("http://127.0.0.1:8787/mcp/health".into()),
        api_key_configured: false,
    }
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
        (
            "task",
            "Create or inspect tasks, or dispatch a specialized RIGA agent",
            false,
        ),
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

pub fn load_workspace_files(root: &Path) -> Vec<FileCandidate> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(name.as_ref(), ".git" | "node_modules" | "target")
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            let path = entry
                .path()
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            Some(FileCandidate {
                name: entry.file_name().to_string_lossy().into_owned(),
                path,
            })
        })
        .take(1_000)
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
                        .unwrap_or_else(|| {
                            if value.get("url").is_some() {
                                "http-stream"
                            } else {
                                "unknown"
                            }
                        })
                        .into(),
                    args: value
                        .get("args")
                        .and_then(serde_json::Value::as_array)
                        .map(|args| {
                            args.iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default(),
                    tools: Vec::new(),
                    transport: Some(
                        value
                            .get("type")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_else(|| {
                                if value.get("url").is_some() {
                                    "http"
                                } else {
                                    "stdio"
                                }
                            })
                            .into(),
                    ),
                    url: value
                        .get("url")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    api_key_configured: value
                        .get("apiKey")
                        .and_then(serde_json::Value::as_str)
                        .is_some(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub aliases: Vec<String>,
    pub purpose: String,
    pub tools: Vec<String>,
    pub model_preference: String,
    pub read_only: bool,
    pub system_rules: Vec<String>,
    pub output_format: Vec<String>,
}

pub fn agent_profiles() -> Vec<AgentProfile> {
    vec![
        agent_profile(
            "explore",
            &["scout", "explorer"],
            true,
            "Fast, read-only codebase reconnaissance with compressed hand-off context.",
            &["read", "glob", "grep", "bash(read-only)"],
            "cheapest/fastest capable model",
            &[
                "Never ask questions; make reasonable assumptions and document them.",
                "Never create, modify, delete files, install packages, or change system state.",
                "Use workspace-relative paths and line numbers whenever possible.",
            ],
            &[
                "Summary",
                "Answer",
                "Files Retrieved",
                "Key Code",
                "Architecture",
                "Start Here",
            ],
        ),
        agent_profile(
            "plan",
            &["planner"],
            true,
            "Turn requirements and exploration findings into a concrete implementation plan.",
            &["read", "glob", "grep", "bash(read-only)"],
            "strong reasoning model",
            &[
                "Never ask questions and never edit files.",
                "Keep steps actionable at file and function level.",
                "State risks and assumptions explicitly.",
            ],
            &[
                "Goal",
                "Plan",
                "Files to Modify",
                "New Files",
                "Risks / Assumptions",
            ],
        ),
        agent_profile(
            "build",
            &["executor", "worker"],
            false,
            "Implement a well-scoped task completely and validate it with repository checks.",
            &["read", "write", "glob", "grep", "bash", "task", "skill"],
            "capable coding model",
            &[
                "Never ask questions; complete the task or document a blocker.",
                "Respect RIGA write and shell capability gates and approval requirements.",
                "After changes, run the project's own validation scripts.",
            ],
            &["Completed", "Files Changed", "Notes"],
        ),
        agent_profile(
            "review",
            &["reviewer"],
            true,
            "Independently review a build result for correctness, security, tests, and maintainability.",
            &["read", "glob", "grep", "bash(read-only)"],
            "strong reasoning model",
            &[
                "Never modify files or change system state.",
                "Inspect git diff, tests, security boundaries, and failure modes.",
                "Report actionable findings with severity and file references.",
            ],
            &["Summary", "Findings", "Validation", "Recommendation"],
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn agent_profile(
    name: &str,
    aliases: &[&str],
    read_only: bool,
    purpose: &str,
    tools: &[&str],
    model_preference: &str,
    system_rules: &[&str],
    output_format: &[&str],
) -> AgentProfile {
    AgentProfile {
        name: name.into(),
        aliases: aliases.iter().map(|alias| (*alias).into()).collect(),
        purpose: purpose.into(),
        tools: tools.iter().map(|tool| (*tool).into()).collect(),
        model_preference: model_preference.into(),
        read_only,
        system_rules: system_rules.iter().map(|rule| (*rule).into()).collect(),
        output_format: output_format.iter().map(|item| (*item).into()).collect(),
    }
}

fn find_agent_profile(name: &str) -> Option<AgentProfile> {
    let normalized = name.trim().to_ascii_lowercase();
    agent_profiles().into_iter().find(|profile| {
        profile.name == normalized || profile.aliases.iter().any(|alias| alias == &normalized)
    })
}

pub fn execute_task(input: &serde_json::Value) -> Result<String, String> {
    let action = input
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("list");
    if matches!(action, "agents" | "agent_list" | "dispatch" | "agent") {
        return execute_agent_action(input, action);
    }
    let store = crate::secure_store::SecureStore::from_env()
        .ok_or("task persistence requires RIGA_TOKEN")?;
    let mut tasks = store.load::<Vec<RigaTask>>("tasks")?.unwrap_or_default();
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

fn execute_agent_action(input: &serde_json::Value, action: &str) -> Result<String, String> {
    if matches!(action, "agents" | "agent_list") {
        return serde_json::to_string_pretty(&agent_profiles()).map_err(|error| error.to_string());
    }
    let name = input
        .get("agent")
        .or_else(|| input.get("name"))
        .and_then(serde_json::Value::as_str)
        .ok_or("task dispatch requires agent")?;
    let profile =
        find_agent_profile(name).ok_or_else(|| format!("unknown agent profile: {name}"))?;
    let request = input
        .get("prompt")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    serde_json::to_string_pretty(&serde_json::json!({
        "dispatch": "accepted",
        "agent": profile,
        "prompt": request,
        "handoff": "The parent RIGA run remains responsible for tool execution and approvals."
    }))
    .map_err(|error| error.to_string())
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
    execute_bash_inner(root, command, None).await
}

pub async fn execute_bash_streaming(
    root: &Path,
    command: &str,
    output_sender: mpsc::Sender<String>,
) -> Result<String, String> {
    if std::env::var("RIGA_ENABLE_SHELL").ok().as_deref() != Some("1") {
        return Err("shell execution is disabled; set RIGA_ENABLE_SHELL=1 for an explicitly trusted local server".into());
    }
    execute_bash_inner(root, command, Some(output_sender)).await
}

async fn execute_bash_inner(
    root: &Path,
    command: &str,
    output_sender: Option<mpsc::Sender<String>>,
) -> Result<String, String> {
    let mut child = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("failed to capture shell stdout")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("failed to capture shell stderr")?;
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut stdout_buffer = [0_u8; 1_024];
    let mut stderr_buffer = [0_u8; 1_024];
    let mut output = String::new();

    while stdout_open || stderr_open {
        tokio::select! {
            read = stdout.read(&mut stdout_buffer), if stdout_open => {
                let bytes_read = read.map_err(|e| e.to_string())?;
                if bytes_read == 0 {
                    stdout_open = false;
                } else {
                    append_shell_output(&mut output, &stdout_buffer[..bytes_read], output_sender.as_ref()).await?;
                }
            }
            read = stderr.read(&mut stderr_buffer), if stderr_open => {
                let bytes_read = read.map_err(|e| e.to_string())?;
                if bytes_read == 0 {
                    stderr_open = false;
                } else {
                    append_shell_output(&mut output, &stderr_buffer[..bytes_read], output_sender.as_ref()).await?;
                }
            }
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    if status.success() {
        Ok(output)
    } else {
        Err(format!("command failed: {output}"))
    }
}

async fn append_shell_output(
    output: &mut String,
    bytes: &[u8],
    output_sender: Option<&mpsc::Sender<String>>,
) -> Result<(), String> {
    const MAX_OUTPUT_CHARS: usize = 20_000;
    let remaining = MAX_OUTPUT_CHARS.saturating_sub(output.chars().count());
    if remaining == 0 {
        return Ok(());
    }
    let chunk = String::from_utf8_lossy(bytes)
        .chars()
        .take(remaining)
        .collect::<String>();
    if chunk.is_empty() {
        return Ok(());
    }
    output.push_str(&chunk);
    if let Some(sender) = output_sender {
        sender
            .send(chunk)
            .await
            .map_err(|_| "shell output stream closed".to_owned())?;
    }
    Ok(())
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
        "agents".into(),
        serde_json::to_value(agent_profiles()).unwrap(),
    );
    result.insert(
        "skills".into(),
        serde_json::to_value(load_skills(root)).unwrap(),
    );
    result.insert(
        "files".into(),
        serde_json::to_value(load_workspace_files(root)).unwrap(),
    );
    result.insert(
        "mcp_servers".into(),
        serde_json::to_value(load_mcp_servers(root)).unwrap(),
    );
    result
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tokio::sync::mpsc;

    #[tokio::test]
    async fn shell_output_is_emitted_before_the_command_result() {
        let (sender, mut receiver) = mpsc::channel(4);
        let result = super::execute_bash_inner(
            Path::new("."),
            "printf stdout; printf stderr >&2",
            Some(sender),
        )
        .await
        .expect("command should succeed");
        let mut chunks = Vec::new();
        while let Some(chunk) = receiver.recv().await {
            chunks.push(chunk);
        }

        assert!(result.contains("stdout"));
        assert!(result.contains("stderr"));
        assert!(chunks.iter().any(|chunk| chunk.contains("stdout")));
        assert!(chunks.iter().any(|chunk| chunk.contains("stderr")));
    }

    #[test]
    fn shell_output_is_capped_at_the_protocol_limit() {
        let mut output = "x".repeat(19_999);
        let bytes = b"abcdef";
        let runtime = tokio::runtime::Runtime::new().expect("runtime should initialize");
        runtime
            .block_on(super::append_shell_output(&mut output, bytes, None))
            .expect("output append should succeed");
        assert_eq!(output.chars().count(), 20_000);
        assert!(output.ends_with('a'));
    }
}
