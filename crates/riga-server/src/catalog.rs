use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
};

static INTERACTIVE_SHELL_INPUTS: OnceLock<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>> =
    OnceLock::new();

/// Send one line of terminal input to the currently running streamed shell tool.
/// The CLI exposes this only after the user explicitly enters shell-input mode.
pub fn send_interactive_shell_input(call_id: &str, text: String) -> bool {
    INTERACTIVE_SHELL_INPUTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|inputs| inputs.get(call_id).cloned())
        .is_some_and(|sender| sender.send(text).is_ok())
}

/// Directories that never hold agent-relevant source and would otherwise swamp
/// a result: dependency trees, build output, VCS internals, and the per-session
/// worktrees the workspace module creates.
const IGNORED_DIRECTORIES: [&str; 4] = [".git", "node_modules", "target", ".riga"];

/// Hard cap on any single tool result handed back to the model. Without it one
/// `glob` or `grep` can consume most of the context window before the model
/// sees anything useful.
pub const MAX_TOOL_RESULT_CHARS: usize = 24_000;
/// A single file read is capped a little higher, but still capped; a generated
/// or minified file must not exhaust the window in one call.
const MAX_READ_CHARS: usize = 40_000;
const MAX_GLOB_MATCHES: usize = 300;
const MAX_GREP_MATCHES: usize = 200;
const MAX_GREP_FILE_BYTES: u64 = 1_048_576;

/// True for a directory the tools must not descend into.
///
/// Depth 0 is the workspace root itself, which is never ignored: only a nested
/// `.git`/`node_modules`/`target` is pruned.
fn is_ignored_entry(entry: &walkdir::DirEntry) -> bool {
    entry.depth() > 0
        && entry.file_type().is_dir()
        && IGNORED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref())
}

/// Bound a tool result before it enters the model context, marking the cut so
/// the model knows the result was shortened rather than complete.
pub fn truncate_tool_result(text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    let mut kept: String = text.chars().take(limit).collect();
    kept.push_str(&format!(
        "\n… truncated: result exceeded {limit} characters"
    ));
    kept
}

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
        (
            "find_symbol",
            "Find symbol definitions with file:line evidence",
            false,
        ),
        (
            "find_callers",
            "Find likely callers of a symbol with file:line evidence",
            false,
        ),
        (
            "find_references",
            "Find references to a symbol with file:line evidence",
            false,
        ),
        (
            "find_tests",
            "Find tests related to a symbol with file:line evidence",
            false,
        ),
        (
            "add_evidence",
            "Attach a claim to a source reference",
            false,
        ),
        ("link_evidence", "Link evidence to a claim", false),
        ("remember", "Create reusable knowledge from a run", false),
        (
            "link_knowledge",
            "Link reusable knowledge to evidence",
            false,
        ),
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
        .filter_entry(|entry| !is_ignored_entry(entry))
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
            &["read", "glob", "grep", "skill", "bash"],
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
            // No `bash`: a planner must not execute. With shell access it ran the
            // plan's own commands and looped on them instead of returning a plan;
            // `explore` and `build` are the profiles that run commands.
            &["read", "glob", "grep", "skill"],
            "strong reasoning model",
            &[
                "Never ask questions and never edit files.",
                "Produce the plan only: do not run shell commands, write files, or dispatch agents. Read and search to inform the plan if needed.",
                "Return the plan as your reply and stop. Do not call `update_plan`/`update_todos` repeatedly; the parent records progress.",
                "You cannot dispatch another agent: the task tool is not available to this read-only profile. Return the plan to the parent orchestrator, which will dispatch build if implementation is needed.",
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
                "Create one file per `write` call, with `path` and `content`; never batch several files into one call.",
                "Run one command per `bash` call, with `command`; do not pass an array of commands.",
                "Write under the workspace root shown below, using its absolute path when a relative path would be ambiguous.",
                "Create the files before running anything that needs them: never run `npm install` (or another install) before the manifest it reads (`package.json`, `requirements.txt`, …) exists.",
                "After changes, run the project's own validation scripts.",
            ],
            &["Completed", "Files Changed", "Notes"],
        ),
        agent_profile(
            "review",
            &["reviewer"],
            true,
            "Independently review a build result for correctness, security, tests, and maintainability.",
            &["read", "glob", "grep", "skill", "bash"],
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

pub fn find_agent_profile(name: &str) -> Option<AgentProfile> {
    let normalized = name.trim().to_ascii_lowercase();
    agent_profiles().into_iter().find(|profile| {
        profile.name == normalized || profile.aliases.iter().any(|alias| alias == &normalized)
    })
}

/// The `task` tool: list subagents, dispatch one, or manage durable task
/// records.
///
/// Dispatch is normally intercepted by the run loop (`is_subagent_dispatch`)
/// and runs a real child; the arm here is only reached when the tool is invoked
/// directly.
pub fn execute_task(input: &serde_json::Value) -> Result<String, String> {
    let action = input
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    match action {
        "agents" | "agent_list" => list_agent_profiles(),
        "dispatch" | "agent" | "run" => dispatch_handoff(input),
        "list" | "inspect" | "create" | "update" => execute_durable_task(input, action),
        "" => Err(
            "task requires an action: \"agents\" lists subagents, \"dispatch\" with `agent` and `prompt` runs one, and create/list/inspect/update manage durable tasks"
                .into(),
        ),
        other => Err(format!(
            "unknown task action `{other}`. Call task with action \"agents\" to list subagents, or action \"dispatch\" with `agent` and `prompt` to run one."
        )),
    }
}

/// A compact list of the subagent profiles, each with the exact call that runs
/// it.
///
/// The full profile dump was large enough to drown the context and vague enough
/// that the model read it as "work done", then reported success without ever
/// writing a file. Naming the dispatch call removes that ambiguity.
fn list_agent_profiles() -> Result<String, String> {
    let agents: Vec<serde_json::Value> = agent_profiles()
        .into_iter()
        .map(|profile| {
            serde_json::json!({
                "name": profile.name,
                "aliases": profile.aliases,
                "purpose": profile.purpose,
                "read_only": profile.read_only,
                "model": profile.model_preference,
                "tools": profile.tools,
                "run_with": format!("task action=dispatch agent={} prompt=<the task>", profile.name),
            })
        })
        .collect();
    serde_json::to_string_pretty(&agents).map_err(|error| error.to_string())
}

/// The direct-call handoff for a dispatch. The run loop intercepts real
/// dispatches before this, so this is a fallback for a client that calls the
/// tool itself.
fn dispatch_handoff(input: &serde_json::Value) -> Result<String, String> {
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
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .ok_or("task dispatch requires a prompt")?;
    serde_json::to_string_pretty(&serde_json::json!({
        "dispatch": "accepted",
        "agent": profile.name,
        "prompt": request,
        "handoff": "The parent RIGA run remains responsible for tool execution and approvals."
    }))
    .map_err(|error| error.to_string())
}

/// Durable task records, used for cross-run bookkeeping.
///
/// These are optional, so they degrade to a neutral message when the session
/// has no secure store. The message deliberately does not name an environment
/// variable: an earlier version said "requires RIGA_TOKEN", the model asked the
/// user for the token, and then began putting it in tool arguments.
fn execute_durable_task(input: &serde_json::Value, action: &str) -> Result<String, String> {
    let Some(store) = crate::secure_store::SecureStore::from_env() else {
        return Err(
            "durable task storage is not configured for this session; use the update_plan and update_todos tools for in-run progress instead"
                .into(),
        );
    };
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
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(truncate_tool_result(text, MAX_READ_CHARS))
}

pub async fn execute_write(root: &Path, path: &str, content: &str) -> Result<String, String> {
    // Authorization is the caller's job: the run loop asks the user before
    // reaching here, so this only performs the write it was authorized to do.
    let path = safe_path(root, path)?;
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
    // Authorization is the caller's job; see `execute_write`.
    execute_bash_inner(root, command, None, None).await
}

pub async fn execute_bash_streaming(
    root: &Path,
    command: &str,
    call_id: String,
    output_sender: mpsc::Sender<String>,
) -> Result<String, String> {
    execute_bash_inner(root, command, Some(output_sender), Some(call_id)).await
}

async fn execute_bash_inner(
    root: &Path,
    command: &str,
    output_sender: Option<mpsc::Sender<String>>,
    interactive_call_id: Option<String>,
) -> Result<String, String> {
    let mut command_builder = Command::new("bash");
    command_builder
        .arg("-lc")
        .arg(command)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if interactive_call_id.is_some() {
        command_builder.stdin(Stdio::piped());
    }
    let mut child = command_builder.spawn().map_err(|e| e.to_string())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("failed to capture shell stdout")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("failed to capture shell stderr")?;
    let mut child_stdin = child.stdin.take();
    let (input_sender, mut input_receiver) = mpsc::unbounded_channel();
    let mut input_open = interactive_call_id.is_some();
    if let Some(call_id) = &interactive_call_id {
        INTERACTIVE_SHELL_INPUTS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|error| error.to_string())?
            .insert(call_id.clone(), input_sender);
    }
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
            input = input_receiver.recv(), if input_open => {
                match input {
                    Some(text) => {
                        if let Some(stdin) = child_stdin.as_mut()
                            && stdin.write_all(text.as_bytes()).await.is_err()
                        {
                            child_stdin = None;
                            input_open = false;
                        }
                    }
                    None => input_open = false,
                }
            }
        }
    }
    if let Some(call_id) = &interactive_call_id
        && let Some(inputs) = INTERACTIVE_SHELL_INPUTS.get()
        && let Ok(mut inputs) = inputs.lock()
    {
        inputs.remove(call_id);
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
    let pattern = pattern.trim();
    let matcher = glob::Pattern::new(pattern)
        .map_err(|error| format!("invalid glob pattern `{pattern}`: {error}"))?;
    let mut matches = Vec::new();
    let mut truncated = false;
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !is_ignored_entry(entry))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let relative = match entry.path().strip_prefix(root) {
            Ok(path) => path.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if matcher.matches(&relative) {
            matches.push(relative);
            if matches.len() >= MAX_GLOB_MATCHES {
                truncated = true;
                break;
            }
        }
    }
    if matches.is_empty() {
        return Ok(format!(
            "no files matched `{pattern}` (ignoring {})",
            IGNORED_DIRECTORIES.join(", ")
        ));
    }
    let mut output = matches.join("\n");
    if truncated {
        output.push_str(&format!("\n… truncated at {MAX_GLOB_MATCHES} matches"));
    }
    Ok(output)
}

pub async fn execute_grep(root: &Path, query: &str) -> Result<String, String> {
    let mut matches = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !is_ignored_entry(entry))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        if entry
            .metadata()
            .map(|metadata| metadata.len() > MAX_GREP_FILE_BYTES)
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(text) = tokio::fs::read_to_string(entry.path()).await else {
            continue;
        };
        for (line_number, line) in text.lines().enumerate() {
            if line.contains(query) {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap_or(entry.path())
                    .display();
                matches.push(format!("{relative}:{}:{line}", line_number + 1));
                if matches.len() >= MAX_GREP_MATCHES {
                    return Ok(format!(
                        "{}\n… truncated at {MAX_GREP_MATCHES} matches",
                        matches.join("\n")
                    ));
                }
            }
        }
    }
    if matches.is_empty() {
        return Ok(format!(
            "no matches for `{query}` (ignoring {})",
            IGNORED_DIRECTORIES.join(", ")
        ));
    }
    Ok(matches.join("\n"))
}

pub async fn execute_repository_query(
    root: &Path,
    tool: &str,
    query: &str,
) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err(format!("{tool} requires a non-empty symbol"));
    }
    let needle = match tool {
        "find_tests" => "test".to_owned(),
        _ => query.to_owned(),
    };
    let mut matches = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !is_ignored_entry(entry))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        if entry
            .metadata()
            .map(|m| m.len() > MAX_GREP_FILE_BYTES)
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(text) = tokio::fs::read_to_string(entry.path()).await else {
            continue;
        };
        for (line, content) in text.lines().enumerate() {
            let relevant = if tool == "find_symbol" {
                content.contains(&format!("fn {query}"))
                    || content.contains(&format!("struct {query}"))
                    || content.contains(&format!("class {query}"))
                    || content.contains(&format!("function {query}"))
            } else if tool == "find_tests" {
                content.contains(&needle) && content.contains(query)
            } else {
                content.contains(&needle)
            };
            if relevant {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap_or(entry.path())
                    .display();
                matches.push(format!("{relative}:{}:{content}", line + 1));
                if matches.len() >= MAX_GREP_MATCHES {
                    return Ok(format!(
                        "{}\n… truncated at {MAX_GREP_MATCHES} matches",
                        matches.join("\n")
                    ));
                }
            }
        }
    }
    Ok(if matches.is_empty() {
        format!("no {tool} matches for `{query}`")
    } else {
        matches.join("\n")
    })
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
            None,
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

    #[tokio::test]
    async fn interactive_shell_input_reaches_process_and_registry_is_cleaned() {
        let call_id = "catalog-test-interactive-shell".to_owned();
        let task_call_id = call_id.clone();
        let (sender, mut receiver) = mpsc::channel(8);
        let execution = tokio::spawn(async move {
            super::execute_bash_streaming(
                Path::new("."),
                r#"read -r answer; printf 'answer=%s\n' "$answer""#,
                task_call_id,
                sender,
            )
            .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if super::send_interactive_shell_input(&call_id, "yes\n".into()) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("interactive shell listener should register");

        let output = execution
            .await
            .expect("shell task should not panic")
            .expect("shell process should succeed");
        assert!(output.contains("answer=yes"));
        while receiver.try_recv().is_ok() {}
        assert!(!super::send_interactive_shell_input(
            &call_id,
            "late\n".into()
        ));
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

    fn scratch_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("apps/riga")).unwrap();
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/some-dep")).unwrap();
        std::fs::write(root.join("apps/riga/package.json"), "{}").unwrap();
        std::fs::write(root.join("scripts/run.sh"), "#!/bin/sh\nprobe_marker\n").unwrap();
        std::fs::write(root.join("node_modules/some-dep/package.json"), "{}").unwrap();
        std::fs::write(
            root.join("node_modules/some-dep/index.js"),
            "probe_marker\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn glob_matches_real_patterns_and_ignores_dependencies() {
        // Reported bug: `**/*` was a suffix match that filled up with
        // node_modules, and `apps/*/package.json` matched nothing.
        let workspace = scratch_workspace();
        let root = workspace.path();

        let everything = super::execute_glob(root, "**/*").unwrap();
        assert!(
            everything.contains("apps/riga/package.json"),
            "{everything}"
        );
        assert!(everything.contains("scripts/run.sh"), "{everything}");
        assert!(!everything.contains("node_modules"), "{everything}");

        let targeted = super::execute_glob(root, "apps/*/package.json").unwrap();
        assert_eq!(targeted, "apps/riga/package.json");

        let missing = super::execute_glob(root, "packages/*/package.json").unwrap();
        assert!(missing.contains("no files matched"), "{missing}");
    }

    #[test]
    fn glob_reports_an_invalid_pattern_instead_of_matching_nothing() {
        let workspace = scratch_workspace();
        let error = super::execute_glob(workspace.path(), "[unclosed").unwrap_err();
        assert!(error.contains("invalid glob pattern"), "{error}");
    }

    #[tokio::test]
    async fn grep_ignores_dependency_directories() {
        let workspace = scratch_workspace();
        let output = super::execute_grep(workspace.path(), "probe_marker")
            .await
            .unwrap();
        assert!(output.contains("scripts/run.sh"), "{output}");
        assert!(!output.contains("node_modules"), "{output}");
    }

    #[tokio::test]
    async fn read_is_capped_and_marks_the_cut() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("big.txt");
        std::fs::write(&path, "x".repeat(super::MAX_READ_CHARS + 1_000)).unwrap();
        let output = super::execute_read(workspace.path(), "big.txt")
            .await
            .unwrap();
        assert!(output.contains("… truncated"), "read result was not capped");
    }

    #[test]
    fn truncate_tool_result_leaves_short_results_untouched() {
        let short = "hello".to_owned();
        assert_eq!(super::truncate_tool_result(short.clone(), 100), short);
        let long = "a".repeat(200);
        let capped = super::truncate_tool_result(long, 100);
        assert!(capped.contains("… truncated"));
        assert_eq!(capped.lines().next().unwrap().chars().count(), 100);
    }

    #[test]
    fn task_lists_agents_compactly_with_how_to_dispatch() {
        let output = super::execute_task(&serde_json::json!({"action": "agents"})).unwrap();
        assert!(output.contains("\"name\": \"explore\""), "{output}");
        assert!(output.contains("action=dispatch"), "{output}");
        // The full profile dump drowned the context and read as "work done".
        assert!(!output.contains("system_rules"), "{output}");
    }

    #[test]
    fn task_rejects_unknown_and_missing_actions_without_naming_a_secret() {
        // The reported bug: `action: start` fell through to the persistence
        // path and reported a missing token, so the model asked the user for it.
        let unknown = super::execute_task(&serde_json::json!({"action": "start"})).unwrap_err();
        assert!(unknown.contains("unknown task action"), "{unknown}");
        assert!(!unknown.contains("RIGA_TOKEN"), "{unknown}");

        let missing = super::execute_task(&serde_json::json!({})).unwrap_err();
        assert!(missing.contains("requires an action"), "{missing}");
        assert!(!missing.contains("RIGA_TOKEN"), "{missing}");
    }

    #[test]
    fn durable_task_actions_degrade_without_naming_an_env_var() {
        // With no secure store configured (the test environment), a durable
        // action must return a neutral message. The old one named RIGA_TOKEN and
        // the model went looking for the secret.
        let error = super::execute_task(&serde_json::json!({"action": "list"})).unwrap_err();
        assert!(!error.contains("RIGA_TOKEN"), "{error}");
        assert!(
            error.contains("update_plan") || error.contains("durable task storage"),
            "{error}"
        );
    }

    #[test]
    fn task_dispatch_accepts_alias_actions_and_name_field() {
        for action in ["agent", "run"] {
            let output = super::execute_task(&serde_json::json!({
                "action": action,
                "name": "scout",
                "prompt": "inspect the workspace"
            }))
            .unwrap();
            assert!(output.contains("\"agent\": \"explore\""), "{output}");
        }
    }

    #[test]
    fn task_dispatch_validates_agent_name_and_prompt() {
        let missing_agent = super::execute_task(&serde_json::json!({
            "action": "dispatch",
            "prompt": "inspect"
        }))
        .unwrap_err();
        assert!(missing_agent.contains("requires agent"), "{missing_agent}");

        let unknown_agent = super::execute_task(&serde_json::json!({
            "action": "dispatch",
            "agent": "unknown",
            "prompt": "inspect"
        }))
        .unwrap_err();
        assert!(
            unknown_agent.contains("unknown agent profile"),
            "{unknown_agent}"
        );

        let missing_prompt = super::execute_task(&serde_json::json!({
            "action": "dispatch",
            "agent": "explore"
        }))
        .unwrap_err();
        assert!(
            missing_prompt.contains("requires a prompt"),
            "{missing_prompt}"
        );
    }

    #[test]
    fn every_profile_alias_resolves_to_its_canonical_name() {
        for (alias, expected) in [
            ("scout", "explore"),
            ("explorer", "explore"),
            ("planner", "plan"),
            ("executor", "build"),
            ("worker", "build"),
            ("reviewer", "review"),
        ] {
            assert_eq!(super::find_agent_profile(alias).unwrap().name, expected);
        }
        assert!(super::find_agent_profile("not-an-agent").is_none());
    }
}
