use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use riga_kernel::events::{RigaEvent, RigaEventEnvelope};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::mpsc;

use crate::mcp::McpRuntime;

/// Which backend serves a run. Defaults to `Remote` so a client written before
/// local models existed keeps working unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    Remote,
    Local,
}

/// Which OpenAI API shape a remote provider speaks.
///
/// `Chat` is the `/chat/completions` contract that OpenAI-compatible gateways
/// (OpenCode Go, vLLM, LiteLLM, and most others) implement. `Responses` is
/// OpenAI's newer `/responses` API. The shape is chosen explicitly rather than
/// inferred from the model name: a compatible gateway answers `/responses` with
/// an HTML 404 even for a `gpt-5*` model, so a name prefix is not a safe signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderApi {
    #[default]
    Chat,
    Responses,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    #[serde(default = "default_reasoning_effort")]
    pub reasoning_effort: String,
    /// `Remote` talks to an OpenAI-compatible HTTP endpoint; `Local` runs an
    /// in-process GGUF through llama.cpp. The endpoint and API key are ignored
    /// for `Local`, which is why the UI can keep sending the same shape.
    #[serde(default)]
    pub kind: ProviderKind,
    /// Which remote API to call. Ignored when `kind` is `Local`.
    #[serde(default)]
    pub api: ProviderApi,
    /// Optional cheaper model for read-only subagents (`explore`/`plan`/
    /// `review`). When unset, subagents use the main model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_model: Option<String>,
}

impl ProviderConfig {
    fn is_local(&self) -> bool {
        self.kind == ProviderKind::Local
    }
}

fn default_reasoning_effort() -> String {
    "low".into()
}

/// How a pending approval was answered.
#[derive(Debug, Clone, Copy)]
struct ApprovalReply {
    approved: bool,
    /// Approve this tool for the rest of the session, not just this call.
    always: bool,
}

/// Resolves tool approvals for one WebSocket session.
///
/// A run suspends inside `request_approval`; the socket's read loop resolves it
/// when the `Approval` frame arrives. `always_allowed` persists across runs in
/// the same session so "always allow" means exactly that.
#[derive(Clone, Default)]
struct ApprovalBroker {
    waiters: std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<String, tokio::sync::oneshot::Sender<ApprovalReply>>,
        >,
    >,
    always_allowed: std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
}

impl ApprovalBroker {
    /// Deliver a decision to the run waiting on `approval_id`. Returns whether
    /// a waiter was found, so a stray or duplicate decision is a no-op.
    async fn resolve(&self, approval_id: &str, reply: ApprovalReply) -> bool {
        let mut waiters = self.waiters.lock().await;
        if let Some(sender) = waiters.remove(approval_id) {
            let _ = sender.send(reply);
            true
        } else {
            false
        }
    }

    async fn is_always_allowed(&self, tool: &str) -> bool {
        self.always_allowed.lock().await.contains(tool)
    }

    async fn always_allow(&self, tool: &str) {
        self.always_allowed.lock().await.insert(tool.to_owned());
    }
}

/// Ask the user to approve a gated tool call, suspending the run until they
/// answer or the socket closes. Returns a denial on timeout so a run can never
/// hang forever on a client that went away.
async fn request_approval(
    broker: &ApprovalBroker,
    trace_sender: &mpsc::Sender<ToolTraceEvent>,
    call_id: &str,
    tool: &str,
    summary: &str,
) -> ApprovalReply {
    static APPROVAL_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let approval_id = format!(
        "approval-{}",
        APPROVAL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    broker
        .waiters
        .lock()
        .await
        .insert(approval_id.clone(), sender);
    let _ = trace_sender
        .send(ToolTraceEvent::Ui(
            riga_kernel::events::RigaEvent::ApprovalRequested {
                approval_id: approval_id.clone(),
                task_id: call_id.to_owned(),
                tool: tool.to_owned(),
                summary: summary.to_owned(),
            },
        ))
        .await;
    let reply = match tokio::time::timeout(std::time::Duration::from_secs(900), receiver).await {
        Ok(Ok(reply)) => reply,
        // Dropped sender (socket closed) or timed out: treat as denied.
        _ => ApprovalReply {
            approved: false,
            always: false,
        },
    };
    broker.waiters.lock().await.remove(&approval_id);
    let _ = trace_sender
        .send(ToolTraceEvent::Ui(
            riga_kernel::events::RigaEvent::ApprovalResolved {
                approval_id,
                approved: reply.approved,
                reason: None,
            },
        ))
        .await;
    reply
}

/// A short, human-readable description of a gated call for the approval card.
fn approval_summary(tool: &str, input: &serde_json::Value) -> String {
    match tool {
        "write" | "edit" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| format!("write {path}"))
            .unwrap_or_else(|| "write a file".to_owned()),
        "bash" | "shell" => input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|command| command.to_owned())
            .unwrap_or_else(|| "run a shell command".to_owned()),
        other => format!("call {other}"),
    }
}

/// Decide whether a tool call may proceed, asking the user when policy requires
/// it. This replaces the old environment-variable write/shell gates.
async fn authorize_tool(
    broker: &ApprovalBroker,
    trace_sender: &mpsc::Sender<ToolTraceEvent>,
    call_id: &str,
    name: &str,
    input: &serde_json::Value,
) -> Result<(), String> {
    let decision =
        riga_kernel::policy::ToolPolicy::default().evaluate(&riga_kernel::policy::ToolRequest {
            tool_name: name.to_owned(),
            risk: riga_kernel::policy::ToolRisk::for_tool(name),
            target: approval_summary(name, input),
            explanation: String::new(),
        });
    match decision {
        riga_kernel::policy::ApprovalDecision::Allow => Ok(()),
        riga_kernel::policy::ApprovalDecision::Deny => Err(format!("`{name}` is not permitted")),
        riga_kernel::policy::ApprovalDecision::RequireApproval => {
            if broker.is_always_allowed(name).await {
                return Ok(());
            }
            let reply = request_approval(
                broker,
                trace_sender,
                call_id,
                name,
                &approval_summary(name, input),
            )
            .await;
            if reply.approved {
                if reply.always {
                    broker.always_allow(name).await;
                }
                Ok(())
            } else {
                Err(format!("the user denied `{name}`"))
            }
        }
    }
}

/// One completed exchange kept for context.
///
/// Persisted per session so a follow-up like "go ahead" arrives with the plan
/// it refers to instead of starting a cold run that asks for the goal again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: String,
    pub content: String,
}

/// How many prior turns are replayed to the model, and the total character
/// budget across them. Bounded so a long session cannot crowd out the request.
pub const MAX_HISTORY_TURNS: usize = 16;
pub const MAX_HISTORY_CHARS: usize = 24_000;

/// Keep the most recent turns that fit both budgets, dropping the oldest first.
pub fn trim_history(turns: &[ConversationTurn]) -> Vec<ConversationTurn> {
    let mut kept: Vec<ConversationTurn> = turns
        .iter()
        .rev()
        .take(MAX_HISTORY_TURNS)
        .cloned()
        .collect();
    kept.reverse();
    let mut total = 0usize;
    let mut start = 0usize;
    for (index, turn) in kept.iter().enumerate().rev() {
        total += turn.content.chars().count();
        if total > MAX_HISTORY_CHARS {
            start = index + 1;
            break;
        }
    }
    kept.split_off(start)
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        client_version: String,
    },
    ConfigureProvider(ProviderConfig),
    StartRun {
        run_id: String,
        session_id: String,
        prompt: String,
    },
    /// Replay the events of a run after a cursor, for a client that
    /// disconnected mid-run and wants to catch up.
    ResumeRun {
        run_id: String,
        #[serde(default)]
        after_sequence: u64,
    },
    CancelRun {
        run_id: String,
    },
    Approval {
        run_id: String,
        approval_id: String,
        approved: bool,
        /// Optional scope from the UI, e.g. `always` to allow this tool for the
        /// rest of the session. `once` (or absent) allows just this call.
        #[serde(default)]
        option: Option<String>,
    },
    Ping {
        nonce: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ready {
        protocol_version: u16,
        server_version: &'static str,
    },
    ProviderConfigured {
        endpoint: String,
        model: String,
        reasoning_effort: String,
        /// The backend runs will actually use. The endpoint and model above are
        /// echoed as stored, so a client switching back to remote keeps them,
        /// but they are ignored when this is `local`.
        kind: ProviderKind,
        /// Which remote API the stored config will call, so the form can show
        /// the current choice after a reload.
        api: ProviderApi,
        /// Optional cheaper model for read-only subagents, echoed for the form.
        #[serde(skip_serializing_if = "Option::is_none")]
        subagent_model: Option<String>,
    },
    Event {
        envelope: RigaEventEnvelope,
    },
    RunCancelled {
        run_id: String,
    },
    ApprovalRecorded {
        run_id: String,
        approval_id: String,
        approved: bool,
    },
    Pong {
        nonce: String,
    },
    Error {
        code: String,
        message: String,
    },
    ToolResult {
        call_id: String,
        name: String,
        ok: bool,
        output: String,
    },
}

pub async fn upgrade(
    socket: WebSocket,
    workspace_root: PathBuf,
    mcp_runtime: McpRuntime,
    local_models: std::sync::Arc<crate::local_model::LocalModelRuntime>,
    transcripts: std::sync::Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, Vec<ConversationTurn>>>,
    >,
    secure_store: Option<std::sync::Arc<crate::secure_store::SecureStore>>,
) {
    let (mut sender, mut receiver) = socket.split();
    let broker = ApprovalBroker::default();
    let mut provider: Option<ProviderConfig> =
        match crate::secure_store::load_json("provider").await {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "provider settings could not be loaded");
                None
            }
        };
    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Hello { .. }) => {
                        if send(
                            &mut sender,
                            ServerMessage::Ready {
                                protocol_version: riga_kernel::PROTOCOL_VERSION,
                                server_version: "0.1.0",
                            },
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                        if let Some(config) = &provider
                            && send(
                                &mut sender,
                                ServerMessage::ProviderConfigured {
                                    endpoint: config.endpoint.clone(),
                                    model: config.model.clone(),
                                    reasoning_effort: config.reasoning_effort.clone(),
                                    kind: config.kind,
                                    api: config.api,
                                    subagent_model: config.subagent_model.clone(),
                                },
                            )
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(ClientMessage::ConfigureProvider(mut config)) => {
                        if config.api_key.trim().is_empty()
                            && let Some(existing) = &provider
                        {
                            config.api_key = existing.api_key.clone();
                        }
                        let model = config.model.clone();
                        let endpoint = config.endpoint.clone();
                        let reasoning_effort = config.reasoning_effort.clone();
                        let kind = config.kind;
                        let api = config.api;
                        let subagent_model = config.subagent_model.clone();
                        if let Err(error) =
                            crate::secure_store::save_json("provider", &config).await
                        {
                            tracing::error!(%error, "provider settings could not be persisted");
                            if send(
                                &mut sender,
                                ServerMessage::Error {
                                    code: "provider_persist_failed".into(),
                                    message: format!(
                                        "Provider settings could not be persisted: {error}"
                                    ),
                                },
                            )
                            .await
                            .is_err()
                            {
                                return;
                            }
                        }
                        provider = Some(config);
                        if send(
                            &mut sender,
                            ServerMessage::ProviderConfigured {
                                endpoint,
                                model,
                                reasoning_effort,
                                kind,
                                api,
                                subagent_model,
                            },
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                    }
                    Ok(ClientMessage::Ping { nonce }) => {
                        let _ = send(&mut sender, ServerMessage::Pong { nonce }).await;
                    }
                    Ok(ClientMessage::Approval {
                        run_id,
                        approval_id,
                        approved,
                        option,
                    }) => {
                        let reply = ApprovalReply {
                            approved,
                            always: option.as_deref() == Some("always"),
                        };
                        broker.resolve(&approval_id, reply).await;
                        let _ = send(
                            &mut sender,
                            ServerMessage::ApprovalRecorded {
                                run_id,
                                approval_id,
                                approved,
                            },
                        )
                        .await;
                    }
                    Ok(ClientMessage::CancelRun { run_id }) => {
                        let _ = send(&mut sender, ServerMessage::RunCancelled { run_id }).await;
                    }
                    Ok(ClientMessage::ResumeRun {
                        run_id,
                        after_sequence,
                    }) => {
                        // Replay a run's journaled events from the client's
                        // cursor, so a reconnect catches up without re-running.
                        match riga_kernel::persistence::EventJournal::open(run_journal_path(
                            &run_id,
                        )) {
                            Ok(journal) => {
                                for replay in journal.after_sequence(after_sequence) {
                                    if send(&mut sender, ServerMessage::Event { envelope: replay })
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                let _ = send(
                                    &mut sender,
                                    ServerMessage::Error {
                                        code: "run_resume_failed".into(),
                                        message: error.message,
                                    },
                                )
                                .await;
                            }
                        }
                    }
                    Ok(ClientMessage::ToolCall {
                        call_id,
                        name,
                        input,
                    }) => {
                        let result =
                            execute_tool(&workspace_root, &mcp_runtime, &name, input, None).await;
                        let (ok, output) = match result {
                            Ok(output) => (true, output),
                            Err(error) => (false, error),
                        };
                        let _ = send(
                            &mut sender,
                            ServerMessage::ToolResult {
                                call_id,
                                name,
                                ok,
                                output,
                            },
                        )
                        .await;
                    }
                    Ok(ClientMessage::StartRun {
                        run_id,
                        session_id,
                        prompt,
                    }) => {
                        let config = match resolve_run_provider(
                            provider.as_ref(),
                            local_models.loaded_file_name().is_some(),
                        ) {
                            Ok(config) => config,
                            Err((code, message)) => {
                                let _ = send(
                                    &mut sender,
                                    ServerMessage::Error {
                                        code: code.into(),
                                        message: message.into(),
                                    },
                                )
                                .await;
                                continue;
                            }
                        };
                        // Fail before opening a run when a local provider is selected
                        // but nothing is loaded, so the user gets an actionable
                        // message instead of an inference error mid-stream.
                        if config.is_local() && local_models.loaded_file_name().is_none() {
                            let _ = send(&mut sender, ServerMessage::Error { code: "local_model_not_loaded".into(), message: "Load a local model in the model manager before starting a run.".into() }).await;
                            continue;
                        }
                        let history = {
                            let guard = transcripts.read().await;
                            trim_history(guard.get(&session_id).map(Vec::as_slice).unwrap_or(&[]))
                        };
                        match send_provider_events(
                            &mut sender,
                            &mut receiver,
                            &broker,
                            &config,
                            &workspace_root,
                            &run_id,
                            &session_id,
                            &prompt,
                            &mcp_runtime,
                            &local_models,
                            &history,
                        )
                        .await
                        {
                            Ok(Some(output)) => {
                                append_turns(
                                    &transcripts,
                                    &secure_store,
                                    &session_id,
                                    &[
                                        ConversationTurn {
                                            role: "user".into(),
                                            content: prompt.clone(),
                                        },
                                        ConversationTurn {
                                            role: "assistant".into(),
                                            content: output,
                                        },
                                    ],
                                )
                                .await;
                            }
                            // The run failed. Keep the user's turn so the session
                            // still has context, but do not invent a reply.
                            Ok(None) => {
                                append_turns(
                                    &transcripts,
                                    &secure_store,
                                    &session_id,
                                    &[ConversationTurn {
                                        role: "user".into(),
                                        content: prompt.clone(),
                                    }],
                                )
                                .await;
                            }
                            // A socket error means the client is gone; stop.
                            Err(_) => return,
                        }
                    }
                    Err(error) => {
                        let _ = send(
                            &mut sender,
                            ServerMessage::Error {
                                code: "invalid_message".into(),
                                message: error.to_string(),
                            },
                        )
                        .await;
                    }
                }
            }
            Message::Close(_) => return,
            Message::Ping(payload) => {
                if sender.send(Message::Pong(payload)).await.is_err() {
                    return;
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
struct ToolTrace {
    call: serde_json::Value,
    name: String,
    output: String,
    ok: bool,
}

#[derive(Debug)]
enum ToolTraceEvent {
    Started(serde_json::Value),
    Output {
        call_id: String,
        delta: String,
    },
    Completed(ToolTrace),
    /// A server-internal tool (plan/todo/task) produced a UI frame rather than
    /// ordinary text output. Forwarded verbatim as its own event.
    Ui(riga_kernel::events::RigaEvent),
}

#[derive(Clone)]
struct ToolOutputStream {
    call_id: String,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
}

impl ToolOutputStream {
    async fn send(&self, delta: String) -> Result<(), String> {
        self.trace_sender
            .send(ToolTraceEvent::Output {
                call_id: self.call_id.clone(),
                delta,
            })
            .await
            .map_err(|_| "tool lifecycle stream closed".to_owned())
    }
}

async fn execute_tool(
    workspace_root: &std::path::Path,
    mcp_runtime: &McpRuntime,
    name: &str,
    input: serde_json::Value,
    output_stream: Option<ToolOutputStream>,
) -> Result<String, String> {
    match name {
        "read" => {
            crate::catalog::execute_read(
                workspace_root,
                input
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("read requires path")?,
            )
            .await
        }
        "write" => {
            crate::catalog::execute_write(
                workspace_root,
                input
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("write requires path")?,
                input
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("write requires content")?,
            )
            .await
        }
        "bash" | "shell" => {
            let command = input
                .get("command")
                .and_then(serde_json::Value::as_str)
                .ok_or("bash requires command")?;
            if let Some(output_stream) = output_stream {
                execute_streaming_bash(workspace_root, command, output_stream).await
            } else {
                crate::catalog::execute_bash(workspace_root, command).await
            }
        }
        "glob" => crate::catalog::execute_glob(
            workspace_root,
            input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .ok_or("glob requires pattern")?,
        ),
        "grep" => {
            crate::catalog::execute_grep(
                workspace_root,
                input
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("grep requires query")?,
            )
            .await
        }
        "web" => {
            crate::catalog::execute_web(
                input
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("web requires url")?,
            )
            .await
        }
        "task" => crate::catalog::execute_task(&input),
        // Plan and todo are server-internal: the model writes the current state
        // and the UI renders it as a pinned element. Nothing leaves the process.
        "update_plan" => {
            let plan: riga_kernel::task::Plan = serde_json::from_value(input)
                .map_err(|error| format!("update_plan arguments are invalid: {error}"))?;
            if let Some(stream) = &output_stream {
                let _ = stream
                    .trace_sender
                    .send(ToolTraceEvent::Ui(
                        riga_kernel::events::RigaEvent::PlanUpdated {
                            plan: Box::new(plan.clone()),
                        },
                    ))
                    .await;
            }
            Ok(format!(
                "Plan updated: {} step(s), currently on step {}.",
                plan.steps.len(),
                plan.active_index + 1
            ))
        }
        "update_todos" => {
            let list: riga_kernel::task::TodoList = serde_json::from_value(input)
                .map_err(|error| format!("update_todos arguments are invalid: {error}"))?;
            if let Some(stream) = &output_stream {
                let _ = stream
                    .trace_sender
                    .send(ToolTraceEvent::Ui(
                        riga_kernel::events::RigaEvent::TodoUpdated {
                            list: Box::new(list.clone()),
                        },
                    ))
                    .await;
            }
            let (done, total) = list.progress();
            Ok(format!("Todo list updated: {done}/{total} complete."))
        }
        "skill" => {
            if let Some(name) = input
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.trim().is_empty())
            {
                crate::catalog::execute_read(workspace_root, &format!("skills/{name}/SKILL.md"))
                    .await
            } else {
                let skills = crate::catalog::load_skills(workspace_root);
                serde_json::to_string_pretty(&skills).map_err(|error| error.to_string())
            }
        }
        _ => mcp_runtime.execute(name, &input).await,
    }
}

async fn execute_streaming_bash(
    workspace_root: &std::path::Path,
    command: &str,
    output_stream: ToolOutputStream,
) -> Result<String, String> {
    let (output_sender, mut output_receiver) = mpsc::channel(1);
    let mut execution = Box::pin(crate::catalog::execute_bash_streaming(
        workspace_root,
        command,
        output_sender,
    ));
    let mut output_open = true;
    loop {
        tokio::select! {
            output = output_receiver.recv(), if output_open => {
                match output {
                    Some(delta) => output_stream.send(delta).await?,
                    None => output_open = false,
                }
            }
            result = &mut execution => {
                while let Ok(delta) = output_receiver.try_recv() {
                    output_stream.send(delta).await?;
                }
                return result;
            }
        }
    }
}

struct AgentResult {
    output: String,
}

/// The per-run entrypoint for the provider loop. It threads the connection, the
/// resolved provider, the workspace and the two runtimes together; bundling them
/// into a struct would only move the same fields behind one more name.
#[allow(clippy::too_many_arguments)]
async fn send_provider_events<S>(
    sender: &mut S,
    receiver: &mut futures_util::stream::SplitStream<WebSocket>,
    broker: &ApprovalBroker,
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    run_id: &str,
    session_id: &str,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    local_models: &std::sync::Arc<crate::local_model::LocalModelRuntime>,
    history: &[ConversationTurn],
) -> Result<Option<String>, S::Error>
where
    S: SinkExt<Message> + Unpin,
{
    // Durable journal for this run. Reopened per run; appends are idempotent on
    // `event_id`, so a retried frame cannot duplicate.
    let mut journal = riga_kernel::persistence::EventJournal::open(run_journal_path(run_id)).ok();
    emit_event(
        sender,
        &mut journal,
        envelope(run_id, session_id, 1, RigaEvent::RunStarted),
    )
    .await?;
    let (trace_sender, mut trace_receiver) = mpsc::channel(1);
    // A leading `@agent` runs that subagent directly; otherwise the
    // orchestrator model decides what to do.
    let mut provider_call: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<AgentResult, String>> + Send>,
    > = if let Some((agent, task)) = leading_agent_mention(prompt) {
        let workspace = workspace_root.to_path_buf();
        let config = config.clone();
        let mcp_runtime = mcp_runtime.clone();
        let trace = trace_sender.clone();
        let broker = broker.clone();
        Box::pin(async move {
            let input = serde_json::json!({
                "action": "dispatch",
                "agent": agent,
                "prompt": task,
                "description": task,
            });
            dispatch_subagent(
                &config,
                &workspace,
                &input,
                &mcp_runtime,
                &trace,
                0,
                &broker,
            )
            .await
            .map(|output| AgentResult { output })
        })
    } else {
        Box::pin(call_openai_compatible(
            config,
            workspace_root,
            prompt,
            mcp_runtime,
            trace_sender,
            local_models,
            history,
            broker,
        ))
    };
    let mut sequence = 2;
    let result = loop {
        tokio::select! {
            trace = trace_receiver.recv() => {
                if let Some(trace) = trace {
                    send_tool_event(sender, &mut journal, run_id, session_id, &mut sequence, trace).await?;
                }
            }
            // While the run is suspended on an approval, keep reading so the
            // decision (or a cancellation or ping) is handled promptly instead
            // of sitting in the socket buffer until the run finishes.
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Approval { approval_id, approved, option, .. }) => {
                                let reply = ApprovalReply {
                                    approved,
                                    always: option.as_deref() == Some("always"),
                                };
                                broker.resolve(&approval_id, reply).await;
                            }
                            Ok(ClientMessage::Ping { nonce }) => {
                                let _ = send(sender, ServerMessage::Pong { nonce }).await;
                            }
                            Ok(ClientMessage::CancelRun { .. }) => {
                                // Cancellation is wired in a later step; the
                                // message is consumed here so it cannot be
                                // mistaken for a protocol error mid-run.
                            }
                            Ok(ClientMessage::ResumeRun { run_id: resume_id, after_sequence }) => {
                                // A client catching up mid-run: replay from the
                                // journal it already has.
                                if let Ok(existing) = riga_kernel::persistence::EventJournal::open(run_journal_path(&resume_id)) {
                                    for replay in existing.after_sequence(after_sequence) {
                                        let _ = send(sender, ServerMessage::Event { envelope: replay }).await;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sender.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => return Ok(None),
                    _ => {}
                }
            }
            result = &mut provider_call => break result,
        }
    };
    while let Ok(trace) = trace_receiver.try_recv() {
        send_tool_event(
            sender,
            &mut journal,
            run_id,
            session_id,
            &mut sequence,
            trace,
        )
        .await?;
    }
    match result {
        Ok(result) => {
            emit_event(
                sender,
                &mut journal,
                envelope(
                    run_id,
                    session_id,
                    sequence,
                    RigaEvent::TextDelta {
                        delta: result.output.clone(),
                    },
                ),
            )
            .await?;
            let output = result.output;
            emit_event(
                sender,
                &mut journal,
                envelope(
                    run_id,
                    session_id,
                    sequence + 1,
                    RigaEvent::RunCompleted {
                        output: output.clone(),
                    },
                ),
            )
            .await?;
            Ok(Some(output))
        }
        Err(error) => {
            emit_event(
                sender,
                &mut journal,
                envelope(
                    run_id,
                    session_id,
                    sequence,
                    RigaEvent::RunFailed { message: error },
                ),
            )
            .await?;
            Ok(None)
        }
    }
}

/// Append turns to a session's history and persist the whole map.
///
/// Persisting the map (rather than appending to a log) mirrors how sessions and
/// the MCP registry are stored, so the durable-workflow work can replace this
/// with a per-turn journal without changing callers.
async fn append_turns(
    transcripts: &std::sync::Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, Vec<ConversationTurn>>>,
    >,
    secure_store: &Option<std::sync::Arc<crate::secure_store::SecureStore>>,
    session_id: &str,
    new_turns: &[ConversationTurn],
) {
    let snapshot = {
        let mut guard = transcripts.write().await;
        guard
            .entry(session_id.to_owned())
            .or_default()
            .extend(new_turns.iter().cloned());
        guard.clone()
    };
    if let Some(store) = secure_store {
        let _ = store.save("transcripts", &snapshot);
    }
}

async fn send_tool_event<S>(
    sender: &mut S,
    journal: &mut Option<riga_kernel::persistence::EventJournal>,
    run_id: &str,
    session_id: &str,
    sequence: &mut u64,
    event: ToolTraceEvent,
) -> Result<(), S::Error>
where
    S: SinkExt<Message> + Unpin,
{
    let event = match event {
        ToolTraceEvent::Started(call) => RigaEvent::ToolCallStarted { call },
        ToolTraceEvent::Output { call_id, delta } => RigaEvent::ToolOutputDelta { call_id, delta },
        ToolTraceEvent::Completed(trace) => {
            let call_id = trace
                .call
                .get("call_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool-call")
                .to_owned();
            RigaEvent::ToolResult {
                result: serde_json::json!({
                    "call_id": call_id,
                    "name": trace.name,
                    "output": trace.output,
                    "ok": trace.ok
                }),
            }
        }
        ToolTraceEvent::Ui(event) => event,
    };
    emit_event(
        sender,
        journal,
        envelope(run_id, session_id, *sequence, event),
    )
    .await?;
    *sequence += 1;
    Ok(())
}

/// Decide what actually serves a run, given the stored provider and whether a
/// local model is loaded. Returns `Err` with `(code, message)` when there is
/// genuinely nothing to run on.
///
/// The local fallback is the point: loading a model is itself a choice of
/// provider, so "download, load, chat" must not additionally require a trip
/// through Settings. The synthesized config carries no endpoint or key because
/// a local run never reads them.
fn resolve_run_provider(
    stored: Option<&ProviderConfig>,
    local_model_loaded: bool,
) -> Result<ProviderConfig, (&'static str, &'static str)> {
    if let Some(config) = stored {
        // A stored *local* provider whose model has since been unloaded is
        // caught by the caller's not-loaded check, which has a more specific
        // remedy than "no provider configured".
        return Ok(config.clone());
    }
    if local_model_loaded {
        return Ok(ProviderConfig {
            endpoint: String::new(),
            api_key: String::new(),
            model: "local".into(),
            reasoning_effort: default_reasoning_effort(),
            kind: ProviderKind::Local,
            // Ignored for a local run; kept explicit so the struct is complete.
            api: ProviderApi::Chat,
            subagent_model: None,
        });
    }
    Err((
        "provider_not_configured",
        "Load a local model or connect a provider in Settings before starting a run.",
    ))
}

#[allow(clippy::too_many_arguments)]
async fn call_openai_compatible(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
    local_models: &std::sync::Arc<crate::local_model::LocalModelRuntime>,
    history: &[ConversationTurn],
    broker: &ApprovalBroker,
) -> Result<AgentResult, String> {
    // A local model short-circuits every HTTP path: there is no endpoint to
    // call and no Responses API, so the API shape must not be consulted.
    if config.is_local() {
        return call_local_model(
            config,
            workspace_root,
            prompt,
            mcp_runtime,
            trace_sender,
            local_models,
            history,
            broker,
        )
        .await;
    }
    match config.api {
        ProviderApi::Responses => {
            call_responses_api(
                config,
                workspace_root,
                prompt,
                mcp_runtime,
                trace_sender,
                history,
            )
            .await
        }
        ProviderApi::Chat => {
            call_chat_with_tools(
                config,
                workspace_root,
                prompt,
                mcp_runtime,
                trace_sender,
                history,
                broker,
            )
            .await
        }
    }
}
async fn call_chat_with_tools(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
    history: &[ConversationTurn],
    broker: &ApprovalBroker,
) -> Result<AgentResult, String> {
    run_chat_loop(
        config,
        workspace_root,
        &coding_agent_system_prompt(),
        None,
        prompt,
        mcp_runtime,
        trace_sender,
        history,
        0,
        broker,
    )
    .await
}

/// The tool-using chat loop. `system_prompt` and `allowed_tools` differ between
/// the orchestrator (all tools) and a dispatched subagent (a restricted set);
/// `depth` bounds subagent nesting.
#[allow(clippy::too_many_arguments)]
async fn run_chat_loop(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    system_prompt: &str,
    allowed_tools: Option<&[String]>,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
    history: &[ConversationTurn],
    depth: usize,
    broker: &ApprovalBroker,
) -> Result<AgentResult, String> {
    let endpoint = if config
        .endpoint
        .trim_end_matches('/')
        .ends_with("/chat/completions")
    {
        config.endpoint.trim_end_matches('/').to_string()
    } else {
        format!("{}/chat/completions", config.endpoint.trim_end_matches('/'))
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    // Resolve the tool list once. The MCP definitions are async and were
    // previously refetched on every turn of the loop.
    let tools = {
        let mut merged =
            crate::mcp::merge_tool_schemas(tool_schemas(), mcp_runtime.tool_definitions().await);
        if let Some(allowed) = allowed_tools {
            merged.retain(|tool| {
                tool.pointer("/function/name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|name| allowed.iter().any(|allowed| allowed == name))
            });
        }
        merged
    };
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt
    })];
    messages.extend(
        history
            .iter()
            .map(|turn| serde_json::json!({ "role": turn.role, "content": turn.content })),
    );
    messages.push(serde_json::json!({ "role": "user", "content": prompt }));
    for _ in 0..24 {
        let mut request = client
            .post(&endpoint)
            .json(&completion_request_body_with_tools(
                &config.model,
                &messages,
                tools.clone(),
            ));
        if !config.api_key.trim().is_empty() {
            request = request.bearer_auth(&config.api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("provider connection failed: {e}"))?;
        let status = response.status();
        let body = response.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "provider returned HTTP {status}: {}",
                redact_body(&body)
            ));
        }
        let value: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("invalid provider response: {e}"))?;
        let message = value
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .cloned()
            .ok_or_else(|| "provider returned no choices".to_owned())?;
        let tool_calls = message
            .get("tool_calls")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        if tool_calls.is_empty() {
            return Ok(AgentResult {
                output: message
                    .get("content")
                    .and_then(content_text)
                    .ok_or_else(|| "provider returned no assistant text".to_owned())?,
            });
        }
        messages.push(message);
        // Parse every call first so the order of results does not depend on the
        // order of execution.
        struct ParsedCall {
            call_id: String,
            name: String,
            input: serde_json::Value,
            call: serde_json::Value,
        }
        let parsed: Vec<ParsedCall> = tool_calls
            .iter()
            .map(|tool_call| -> Result<ParsedCall, String> {
                let call_id = tool_call
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("tool-call")
                    .to_owned();
                let function = tool_call
                    .get("function")
                    .ok_or("provider returned malformed tool call")?;
                let name = function
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("provider tool call has no name")?
                    .to_owned();
                let arguments = function
                    .get("arguments")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("{}");
                let input: serde_json::Value = serde_json::from_str(arguments)
                    .map_err(|error| format!("invalid arguments for {name}: {error}"))?;
                let call = serde_json::json!({
                    "call_id": call_id,
                    "name": name,
                    "arguments": input,
                });
                Ok(ParsedCall {
                    call_id,
                    name,
                    input,
                    call,
                })
            })
            .collect::<Result<_, _>>()?;
        for call in &parsed {
            trace_sender
                .send(ToolTraceEvent::Started(call.call.clone()))
                .await
                .map_err(|_| "tool lifecycle stream closed")?;
        }
        let mut results: Vec<Option<(bool, String)>> = (0..parsed.len()).map(|_| None).collect();
        // Sibling subagent dispatches run concurrently, so a fan-out is actually
        // parallel and the UI can show the workers progressing together.
        let dispatch_indices: Vec<usize> = parsed
            .iter()
            .enumerate()
            .filter(|(_, call)| call.name == "task" && is_subagent_dispatch(&call.input))
            .map(|(index, _)| index)
            .collect();
        if dispatch_indices.len() > 1 {
            let futures = dispatch_indices.iter().map(|&index| {
                let call = &parsed[index];
                let trace = trace_sender.clone();
                let broker = broker.clone();
                let agent = call
                    .input
                    .get("agent")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("subagent")
                    .to_owned();
                async move {
                    let permitted =
                        authorize_tool(&broker, &trace, &call.call_id, "task", &call.input).await;
                    let result = match permitted {
                        Err(message) => Err(message),
                        Ok(()) => dispatch_subagent(
                            config,
                            workspace_root,
                            &call.input,
                            mcp_runtime,
                            &trace,
                            depth,
                            &broker,
                        )
                        .await
                        .map(|output| format!("[{agent} subagent result]\n{output}")),
                    };
                    (index, result)
                }
            });
            for (index, result) in futures_util::future::join_all(futures).await {
                results[index] = Some(match result {
                    Ok(output) => (true, output),
                    Err(error) => (false, format!("tool error: {error}")),
                });
            }
        }
        for (index, call) in parsed.iter().enumerate() {
            let (ok, output) = if let Some(done) = results[index].take() {
                done
            } else {
                let permitted = authorize_tool(
                    broker,
                    &trace_sender,
                    &call.call_id,
                    &call.name,
                    &call.input,
                )
                .await;
                let result = match permitted {
                    // A denied gated call never reaches the tool; the model gets
                    // a clear refusal it can adapt to.
                    Err(message) => Err(message),
                    Ok(()) if call.name == "task" && is_subagent_dispatch(&call.input) => {
                        let agent = call
                            .input
                            .get("agent")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("subagent");
                        Box::pin(dispatch_subagent(
                            config,
                            workspace_root,
                            &call.input,
                            mcp_runtime,
                            &trace_sender,
                            depth,
                            broker,
                        ))
                        .await
                        .map(|output| format!("[{agent} subagent result]\n{output}"))
                    }
                    Ok(()) => {
                        execute_tool(
                            workspace_root,
                            mcp_runtime,
                            &call.name,
                            call.input.clone(),
                            Some(ToolOutputStream {
                                call_id: call.call_id.clone(),
                                trace_sender: trace_sender.clone(),
                            }),
                        )
                        .await
                    }
                };
                match result {
                    Ok(output) => (true, output),
                    Err(error) => (false, format!("tool error: {error}")),
                }
            };
            let trace = ToolTrace {
                call: call.call.clone(),
                name: call.name.clone(),
                output: output.clone(),
                ok,
            };
            trace_sender
                .send(ToolTraceEvent::Completed(trace))
                .await
                .map_err(|_| "tool lifecycle stream closed")?;
            // The raw result is bounded before it enters the model's context: a
            // single glob or read can otherwise consume the window. The full
            // output still went to the UI through the trace above.
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call.call_id,
                "content": crate::catalog::truncate_tool_result(output, crate::catalog::MAX_TOOL_RESULT_CHARS),
            }));
        }
    }
    Err("provider exceeded the maximum tool-call turns".into())
}

/// True when a `task` call is asking to run a subagent rather than to list or
/// create a durable task record.
fn is_subagent_dispatch(input: &serde_json::Value) -> bool {
    matches!(
        input.get("action").and_then(serde_json::Value::as_str),
        Some("dispatch") | Some("agent")
    ) && input
        .get("agent")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|agent| !agent.trim().is_empty())
}

/// Detect a leading `@agent` mention and return the canonical agent name with
/// the remaining instruction.
///
/// This makes the composer's `@explore`/`@plan`/`@build`/`@review` insert
/// deterministic: the named subagent runs directly instead of the model having
/// to notice the mention and choose to dispatch.
fn leading_agent_mention(prompt: &str) -> Option<(String, String)> {
    let rest = prompt.trim_start().strip_prefix('@')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next()?.trim();
    let task = parts.next().unwrap_or("").trim();
    if name.is_empty() || task.is_empty() {
        return None;
    }
    crate::catalog::find_agent_profile(name).map(|profile| (profile.name, task.to_owned()))
}

/// Process-wide counter for subagent task ids.
static TASK_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Run a subagent to completion and return its result as the tool result.
///
/// The child gets its own context (empty history), the profile's system prompt,
/// and a restricted tool set, so its file reads never enter the orchestrator's
/// window — only the summary does. `TaskStarted`/`TaskCompleted` frames let the
/// UI render the delegation.
async fn dispatch_subagent(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    input: &serde_json::Value,
    mcp_runtime: &McpRuntime,
    trace_sender: &mpsc::Sender<ToolTraceEvent>,
    depth: usize,
    broker: &ApprovalBroker,
) -> Result<String, String> {
    if depth + 1 > riga_kernel::task::MAX_TASK_DEPTH {
        return Err(format!(
            "subagents may not nest beyond {} levels",
            riga_kernel::task::MAX_TASK_DEPTH
        ));
    }
    let agent = input
        .get("agent")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("task dispatch requires an agent name")?;
    let prompt = input
        .get("prompt")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("task dispatch requires a prompt")?;
    let profile = crate::catalog::find_agent_profile(agent)
        .ok_or_else(|| format!("unknown agent `{agent}`"))?;
    let task_id = format!(
        "task-{}",
        TASK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let description: String = input
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(prompt)
        .chars()
        .take(120)
        .collect();
    // Read-only recon can run on a cheaper model when one is configured; a
    // mutating profile keeps the main model.
    let child_model = subagent_model_for(config, &profile);
    let child_config = if child_model == config.model {
        config.clone()
    } else {
        let mut clone = config.clone();
        clone.model = child_model.clone();
        clone
    };
    let started = riga_kernel::task::TaskRecord {
        id: task_id.clone(),
        parent_id: Some(format!("depth-{depth}")),
        agent: profile.name.clone(),
        description: description.clone(),
        model: child_model,
        state: riga_kernel::task::TaskState::Running,
        started_at: "now".into(),
        result: None,
    };
    let _ = trace_sender
        .send(ToolTraceEvent::Ui(
            riga_kernel::events::RigaEvent::TaskStarted {
                task: Box::new(started),
            },
        ))
        .await;

    let allowed = allowed_tools_for(&profile);
    let outcome = Box::pin(run_chat_loop(
        &child_config,
        workspace_root,
        &subagent_system_prompt(&profile),
        Some(&allowed),
        prompt,
        mcp_runtime,
        trace_sender.clone(),
        &[],
        depth + 1,
        broker,
    ))
    .await;

    let (ok, result) = match outcome {
        Ok(result) => (true, result.output),
        Err(error) => (false, error),
    };
    let _ = trace_sender
        .send(ToolTraceEvent::Ui(
            riga_kernel::events::RigaEvent::TaskCompleted {
                task_id,
                ok,
                result: result.clone(),
            },
        ))
        .await;
    if ok {
        Ok(result)
    } else {
        Err(format!("{} subagent failed: {result}", profile.name))
    }
}

/// The system prompt for a dispatched subagent, built from its profile rules and
/// expected output sections.
fn subagent_system_prompt(profile: &crate::catalog::AgentProfile) -> String {
    let mut prompt = format!(
        "You are the RIGA `{}` subagent. {}\n\nRules:\n",
        profile.name, profile.purpose
    );
    for rule in &profile.system_rules {
        prompt.push_str(&format!("- {rule}\n"));
    }
    prompt.push_str(
        "\nYou were dispatched by the orchestrator: work autonomously, do not ask the user questions, and return a concise result it can use.\n",
    );
    if !profile.output_format.is_empty() {
        prompt.push_str("Use these sections:\n");
        for section in &profile.output_format {
            prompt.push_str(&format!("- {section}\n"));
        }
    }
    prompt
}

/// The tools a subagent may call. A read-only profile never gets write, shell,
/// or nested dispatch, so it cannot exceed its remit by accident.
fn allowed_tools_for(profile: &crate::catalog::AgentProfile) -> Vec<String> {
    let mut allowed = vec!["update_plan".to_owned(), "update_todos".to_owned()];
    if profile.read_only {
        allowed.extend(["read", "glob", "grep", "skill"].map(str::to_owned));
        return allowed;
    }
    for tool in &profile.tools {
        let base = tool.split('(').next().unwrap_or(tool).trim();
        if !base.is_empty() {
            allowed.push(base.to_owned());
        }
    }
    allowed
}

/// The model a subagent runs on. A configured `subagent_model` applies only to
/// read-only recon profiles; a mutating profile keeps the main model so writes
/// are never handed to a model chosen for cheapness.
fn subagent_model_for(config: &ProviderConfig, profile: &crate::catalog::AgentProfile) -> String {
    match config.subagent_model.as_deref().map(str::trim) {
        Some(model) if profile.read_only && !model.is_empty() => model.to_owned(),
        _ => config.model.clone(),
    }
}
const LOCAL_MAX_TURNS: usize = 8;

/// Tokens requested per local turn before `resolve_max_tokens` shrinks it to the
/// room the prompt leaves. Deliberately modest: a 1.5B model at 4k tokens on CPU
/// is minutes of work, and the tool loop re-decodes the history every turn.
const LOCAL_MAX_TOKENS: u32 = 1024;

#[derive(Debug, PartialEq)]
struct ParsedToolCall {
    name: String,
    arguments: serde_json::Value,
}

/// Extract `<tool_call>{...}</tool_call>` blocks and return them alongside the
/// prose with the blocks removed.
///
/// Two details matter for small models. First, several shapes exist in the wild
/// for the arguments key (`arguments`, `parameters`, `input`, or a bare object),
/// so all are accepted. Second, prose around a tool call must survive, so blocks
/// are cut out of the reply rather than the whole reply being discarded. The
/// two passes share one scan: the first collects calls, the second strips the
/// blocks, so they can never disagree about which blocks were recognised.
fn parse_local_tool_calls(text: &str) -> (String, Vec<ParsedToolCall>) {
    const OPEN: &str = "<tool_call>";
    const CLOSE: &str = "</tool_call>";
    let mut calls = Vec::new();
    let mut prose = String::with_capacity(text.len());
    let mut scan = text;
    // Each iteration consumes one well-formed block, emitting the text before it
    // and skipping past the block itself.
    while let Some(start) = scan.find(OPEN) {
        let after_open = &scan[start + OPEN.len()..];
        // An unterminated block means the model stopped mid-call; keep the
        // remainder as prose instead of dropping the reply.
        let Some(end) = after_open.find(CLOSE) else {
            prose.push_str(scan);
            return (prose.trim().to_owned(), calls);
        };
        let raw = after_open[..end].trim();
        if let Some(call) = parse_one_local_tool_call(raw) {
            calls.push(call);
        }
        prose.push_str(&scan[..start]);
        scan = &after_open[end + CLOSE.len()..];
    }
    prose.push_str(scan);
    (prose.trim().to_owned(), calls)
}

fn parse_one_local_tool_call(raw: &str) -> Option<ParsedToolCall> {
    let candidate: serde_json::Value = serde_json::from_str(raw).ok()?;
    let name = candidate
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())?
        .to_owned();
    let arguments = candidate
        .get("arguments")
        .or_else(|| candidate.get("parameters"))
        .or_else(|| candidate.get("input"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    // Small models often emit the arguments as a JSON *string* containing the
    // object. Unwrap that instead of failing the run over a quoting slip.
    let arguments = match arguments {
        serde_json::Value::String(ref inner) => serde_json::from_str(inner).unwrap_or(arguments),
        other => other,
    };
    Some(ParsedToolCall { name, arguments })
}

/// Tool instructions for a local model, appended to the coding-agent prompt.
///
/// The curated GGUFs are general-purpose instruction models without a native
/// tool-call template, so the protocol is stated explicitly. `<tool_call>` is the
/// Hermes/Qwen convention these checkpoints were tuned on, which is why it is
/// used here instead of inventing a new one.
fn local_tool_instructions(definitions: &[rig_core::completion::ToolDefinition]) -> String {
    let mut text = String::from(
        "\n\nYou can call tools. To call exactly one, reply with this and nothing else:\n<tool_call>{\"name\": \"TOOL_NAME\", \"arguments\": {}}</tool_call>\n\nAvailable tools:\n",
    );
    for definition in definitions {
        let name = definition.name.clone();
        let description = definition.description.clone();
        let parameters =
            serde_json::to_string(&definition.parameters).unwrap_or_else(|_| "{}".to_owned());
        text.push_str(&format!(
            "- {name}: {description}\n  arguments: {parameters}\n"
        ));
    }
    text.push_str(
        "\nCall one tool per reply and wait for its result. When you have the final \
         answer, reply with plain prose and no tool_call block.\n",
    );
    text
}

#[allow(clippy::too_many_arguments)]
async fn call_local_model(
    // Kept for symmetry with the other `call_*` backends; a local run uses the
    // loaded GGUF rather than anything in the provider config.
    _config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
    local_models: &std::sync::Arc<crate::local_model::LocalModelRuntime>,
    history: &[ConversationTurn],
    broker: &ApprovalBroker,
) -> Result<AgentResult, String> {
    let definitions = mcp_runtime.tool_definitions().await;
    let system = format!(
        "{}{}",
        coding_agent_system_prompt(),
        local_tool_instructions(&definitions)
    );
    let mut messages = vec![crate::local_model::ChatMessage {
        role: "system".into(),
        content: system,
    }];
    messages.extend(history.iter().map(|turn| crate::local_model::ChatMessage {
        role: turn.role.clone(),
        content: turn.content.clone(),
    }));
    messages.push(crate::local_model::ChatMessage {
        role: "user".into(),
        content: prompt.to_owned(),
    });
    let mut final_text = String::new();
    for turn in 0..LOCAL_MAX_TURNS {
        // Generation is blocking C, so it runs on the blocking pool. The future
        // is awaited directly: nothing else in this task needs the executor, and
        // a dropped run would otherwise leave a context mid-decode.
        let request = messages.clone();
        let runtime = local_models.clone();
        // Run-scoped stop flag. `CancelRun` cannot reach it yet (see the note at
        // call site), so it is currently always false; passing it explicitly
        // keeps the generate contract intact for when cancellation is wired.
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let text = tokio::task::spawn_blocking(move || {
            runtime.generate(&request, LOCAL_MAX_TOKENS, &cancel, |_delta| {})
        })
        .await
        .map_err(|error| format!("local inference worker failed: {error}"))??;
        let (prose, tool_calls) = parse_local_tool_calls(&text);
        if !prose.is_empty() {
            final_text = prose.clone();
        }
        if tool_calls.is_empty() {
            return Ok(AgentResult {
                output: if final_text.is_empty() {
                    text
                } else {
                    final_text
                },
            });
        }
        // Record the assistant turn so the model can see what it asked for.
        messages.push(crate::local_model::ChatMessage {
            role: "assistant".into(),
            content: text,
        });
        for (index, call) in tool_calls.iter().enumerate() {
            let call_id = format!("local-{turn}-{index}");
            let payload = serde_json::json!({
                "call_id": call_id,
                "name": call.name,
                "arguments": call.arguments,
            });
            trace_sender
                .send(ToolTraceEvent::Started(payload.clone()))
                .await
                .map_err(|_| "tool lifecycle stream closed")?;
            let permitted =
                authorize_tool(broker, &trace_sender, &call_id, &call.name, &call.arguments).await;
            let result = match permitted {
                Err(message) => Err(message),
                Ok(()) => {
                    execute_tool(
                        workspace_root,
                        mcp_runtime,
                        &call.name,
                        call.arguments.clone(),
                        Some(ToolOutputStream {
                            call_id: call_id.clone(),
                            trace_sender: trace_sender.clone(),
                        }),
                    )
                    .await
                }
            };
            let (ok, output) = match result {
                Ok(output) => (true, output),
                Err(error) => (false, format!("tool error: {error}")),
            };
            trace_sender
                .send(ToolTraceEvent::Completed(ToolTrace {
                    call: payload,
                    name: call.name.clone(),
                    output: output.clone(),
                    ok,
                }))
                .await
                .map_err(|_| "tool lifecycle stream closed")?;
            messages.push(crate::local_model::ChatMessage {
                role: "tool".into(),
                content: format!(
                    "Result from {}: {}",
                    call.name,
                    crate::catalog::truncate_tool_result(
                        output,
                        crate::catalog::MAX_TOOL_RESULT_CHARS
                    )
                ),
            });
        }
    }
    Err("local model exceeded the maximum tool-call turns".into())
}

async fn call_responses_api(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
    history: &[ConversationTurn],
) -> Result<AgentResult, String> {
    let endpoint = if config
        .endpoint
        .trim_end_matches('/')
        .ends_with("/responses")
    {
        config.endpoint.trim_end_matches('/').to_string()
    } else {
        format!("{}/responses", config.endpoint.trim_end_matches('/'))
    };
    let effort = match config.reasoning_effort.as_str() {
        "medium" | "high" => config.reasoning_effort.as_str(),
        _ => "low",
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    // The Responses API accepts `input` as a string or a list of messages. Build
    // the list when there is history so prior turns are not lost.
    let current = format!(
        "{}\n\nUser request:\n{}",
        coding_agent_system_prompt(),
        prompt
    );
    let mut input = if history.is_empty() {
        serde_json::json!(current)
    } else {
        let mut items: Vec<serde_json::Value> = history
            .iter()
            .map(|turn| serde_json::json!({ "role": turn.role, "content": turn.content }))
            .collect();
        items.push(serde_json::json!({ "role": "user", "content": current }));
        serde_json::json!(items)
    };
    let mut previous_response_id: Option<String> = None;
    for _ in 0..24 {
        let mut body = serde_json::json!({
            "model": config.model,
            "input": input,
            "max_output_tokens": 1024,
            "reasoning": { "effort": effort },
            "tools": responses_tool_schemas(&mcp_runtime.tool_definitions().await),
        });
        if let Some(id) = &previous_response_id {
            body["previous_response_id"] = serde_json::Value::String(id.clone());
        }
        let mut request = client.post(&endpoint).json(&body);
        if !config.api_key.trim().is_empty() {
            request = request.bearer_auth(&config.api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("provider connection failed: {e}"))?;
        let status = response.status();
        let raw = response.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "provider Responses API returned HTTP {status}: {}",
                redact_body(&raw)
            ));
        }
        let response: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("invalid provider Responses API response: {e}"))?;
        let calls = response
            .get("output")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| {
                item.get("type").and_then(serde_json::Value::as_str) == Some("function_call")
            })
            .collect::<Vec<_>>();
        if calls.is_empty() {
            return Ok(AgentResult {
                output: extract_response_text(&response)?,
            });
        }
        previous_response_id = Some(
            response
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or("provider response has no id for tool continuation")?
                .to_owned(),
        );
        let mut outputs = Vec::new();
        for call in calls {
            let name = call
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or("provider function call has no name")?;
            let arguments = call
                .get("arguments")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("{}");
            let input_value: serde_json::Value = serde_json::from_str(arguments)
                .map_err(|e| format!("invalid arguments for {name}: {e}"))?;
            let call_id = call
                .get("call_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool-call");
            let call_json =
                serde_json::json!({"call_id": call_id, "name": name, "arguments": input_value});
            trace_sender
                .send(ToolTraceEvent::Started(call_json.clone()))
                .await
                .map_err(|_| "tool lifecycle stream closed")?;
            let result = execute_tool(
                workspace_root,
                mcp_runtime,
                name,
                input_value.clone(),
                Some(ToolOutputStream {
                    call_id: call_id.to_owned(),
                    trace_sender: trace_sender.clone(),
                }),
            )
            .await;
            let (ok, output) = match result {
                Ok(output) => (true, output),
                Err(error) => (false, format!("tool error: {error}")),
            };
            let trace = ToolTrace {
                call: call_json,
                name: name.into(),
                output: output.clone(),
                ok,
            };
            trace_sender
                .send(ToolTraceEvent::Completed(trace))
                .await
                .map_err(|_| "tool lifecycle stream closed")?;
            outputs.push(serde_json::json!({"type":"function_call_output", "call_id": call_id, "output": output}));
        }
        input = serde_json::Value::Array(outputs);
    }
    Err("provider exceeded the maximum Responses tool-call turns".into())
}

fn extract_response_text(response: &serde_json::Value) -> Result<String, String> {
    if let Some(text) = response
        .get("output_text")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty())
    {
        return Ok(text.to_owned());
    }
    let mut refusals = Vec::new();
    let output = response
        .get("output")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("message"))
        .filter_map(|item| item.get("content").and_then(serde_json::Value::as_array))
        .flatten()
        .filter_map(
            |part| match part.get("type").and_then(serde_json::Value::as_str) {
                Some("output_text") => part
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                Some("refusal") => {
                    if let Some(text) = part.get("refusal").and_then(serde_json::Value::as_str) {
                        refusals.push(text.to_owned());
                    }
                    None
                }
                _ => None,
            },
        )
        .collect::<Vec<_>>()
        .join("");
    if !output.is_empty() {
        return Ok(output);
    }
    if !refusals.is_empty() {
        return Err(format!(
            "provider refused the request: {}",
            refusals.join(" ")
        ));
    }
    let status = response
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    Err(format!(
        "provider Responses API returned no output text (status: {status})"
    ))
}

fn responses_tool_schemas(mcp: &[rig_core::completion::ToolDefinition]) -> Vec<serde_json::Value> {
    crate::mcp::merge_response_tool_schemas(tool_schemas(), mcp.to_vec())
}

/// Build a chat request from an already-resolved tool list, so a subagent can
/// run with a restricted subset.
fn completion_request_body_with_tools(
    model: &str,
    messages: &[serde_json::Value],
    tools: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "max_completion_tokens": 2048,
        "tools": tools,
        "tool_choice": "auto",
    })
}

fn tool_schemas() -> Vec<serde_json::Value> {
    vec![
        function_schema(
            "read",
            "Read a UTF-8 file inside the workspace",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        ),
        function_schema(
            "write",
            "Write a UTF-8 file; requires approval and the server write gate",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        ),
        function_schema(
            "glob",
            "Find workspace files by glob pattern (for example `src/**/*.ts`). Dependency and build directories are ignored.",
            serde_json::json!({"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"]}),
        ),
        function_schema(
            "grep",
            "Search text in workspace files",
            serde_json::json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
        ),
        function_schema(
            "web",
            "Fetch a public HTTPS page",
            serde_json::json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}),
        ),
        function_schema(
            "bash",
            "Run a workspace shell command; disabled unless explicitly enabled",
            serde_json::json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
        ),
        function_schema(
            "task",
            "Run a specialized subagent to completion and return its result. Use action \"dispatch\" with an agent (explore, plan, build, review) and a prompt; action \"agents\" lists them.",
            serde_json::json!({"type":"object","properties":{"action":{"type":"string","enum":["list","inspect","create","update","agents","agent_list","dispatch","agent"]},"agent":{"type":"string","enum":["explore","plan","build","review","scout","planner","executor","worker","reviewer"]},"prompt":{"type":"string"},"description":{"type":"string"},"name":{"type":"string"},"title":{"type":"string"},"status":{"type":"string"},"task_id":{"type":"string"}},"required":[]}),
        ),
        function_schema(
            "skill",
            "Load a named repository skill document, or list all available skills when name is omitted",
            serde_json::json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
        ),
        function_schema(
            "update_plan",
            "Record or revise the plan you are working through. Call it once after exploring, then again whenever you move to a new step.",
            serde_json::json!({"type":"object","properties":{"title":{"type":"string"},"steps":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"label":{"type":"string"},"description":{"type":"string"}},"required":["id","label"]}},"active_index":{"type":"integer"}},"required":["title","steps","active_index"]}),
        ),
        function_schema(
            "update_todos",
            "Replace your visible working list with the current items. Call it when work is discovered, started, finished, fails, or is dropped.",
            serde_json::json!({"type":"object","properties":{"title":{"type":"string"},"revision":{"type":"integer"},"items":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"text":{"type":"string"},"description":{"type":"string"},"status":{"type":"string","enum":["pending","active","done","failed","cancelled"]},"reason":{"type":"string"}},"required":["id","text","status"]}}},"required":["items"]}),
        ),
    ]
}

fn function_schema(
    name: &str,
    description: &str,
    mut parameters: serde_json::Value,
) -> serde_json::Value {
    if parameters
        .get("required")
        .and_then(serde_json::Value::as_array)
        .is_some_and(Vec::is_empty)
        && let Some(object) = parameters.as_object_mut()
    {
        object.remove("required");
    }
    serde_json::json!({"type":"function","function":{"name":name,"description":description,"parameters":parameters}})
}

fn content_text(content: &serde_json::Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .filter(|text| !text.is_empty())
}

/// The system prompt, built per run.
fn coding_agent_system_prompt() -> String {
    let workspace = crate::catalog::workspace_root();
    let mut prompt = String::from(
        "You are RIGA, a coding agent operating inside the configured workspace. \
For requests that create, modify, inspect, run, or validate software, use the available tools instead of only describing commands or code. \
Work in small observable steps: inspect first, then make the smallest change, then validate. \
Never claim a file or command succeeded unless a tool result confirms it.\n\n\
Tools:\n\
- `glob` takes a real glob pattern relative to the workspace (`src/**/*.ts`, `apps/*/package.json`). Dependency and build directories are already ignored.\n\
- `grep` searches file contents; `read` reads one file. Read a file before editing it.\n\
- `task` dispatches a specialized subagent. Its `action: \"agents\"` form lists them. \
When the user addresses an agent with `@explore`, `@plan`, `@build`, or `@review`, dispatch that agent with the `task` tool and the matching `agent` argument rather than doing the work yourself when the profile's remit fits.\n\
- `update_plan` records the checklist you are working through and `update_todos` keeps your working list current. \
Call `update_plan` once right after exploring, then `update_todos` as work is discovered, started, finished, fails, or is dropped, instead of narrating progress in prose.\n\
- When the user names a specific MCP server, prefer its qualified tool alias beginning with `mcp_` (for example `mcp_riga_health_stdio_health`) over a built-in.\n\n",
    );
    prompt.push_str(&format!("Workspace root: {}\n", workspace.display()));
    prompt.push_str(
        "Writing files and running shell commands pause for the user's approval before they execute. \
That is expected: just call the tool and wait for the result. Never ask the user to approve in chat, \
and never claim a write or command succeeded before its tool result confirms it. \
If an approval is denied, adapt or summarize what you completed rather than retrying the same call.\n",
    );
    prompt.push_str(
        "Keep the final response concise and summarize the actual files and validation results.",
    );
    prompt
}

fn redact_body(body: &str) -> String {
    body.chars().take(300).collect()
}

async fn send<S>(sender: &mut S, message: ServerMessage) -> Result<(), S::Error>
where
    S: SinkExt<Message> + Unpin,
{
    let payload = serde_json::to_string(&message).expect("WebSocket message is serializable");
    sender.send(Message::Text(payload.into())).await
}

fn envelope(run_id: &str, session_id: &str, sequence: u64, event: RigaEvent) -> RigaEventEnvelope {
    RigaEventEnvelope {
        protocol_version: riga_kernel::PROTOCOL_VERSION,
        event_id: format!("{run_id}-{sequence}"),
        session_id: session_id.into(),
        run_id: run_id.into(),
        sequence,
        timestamp: "now".into(),
        event,
    }
}

/// Sanitize a client-supplied run id into a safe file-name component.
fn sanitize_run_id(run_id: &str) -> String {
    run_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// Durable journal for one run, under the data directory.
fn run_journal_path(run_id: &str) -> std::path::PathBuf {
    crate::secure_store::data_root()
        .join("runs")
        .join(format!("run-{}.json", sanitize_run_id(run_id)))
}

/// Send an event and, when a journal is open, durably record it first so a
/// reconnect can replay it. A journal failure is logged, never fatal: losing
/// durability is worse than losing the run, but not worse than losing the run.
async fn emit_event<S>(
    sender: &mut S,
    journal: &mut Option<riga_kernel::persistence::EventJournal>,
    envelope: RigaEventEnvelope,
) -> Result<(), S::Error>
where
    S: SinkExt<Message> + Unpin,
{
    if let Some(journal) = journal.as_mut()
        && let Err(error) = journal.append(envelope.clone())
    {
        tracing::warn!(?error, "run event could not be journaled");
    }
    send(sender, ServerMessage::Event { envelope }).await
}

#[cfg(test)]
mod tests {
    use super::{
        ClientMessage, ParsedToolCall, ProviderApi, ProviderConfig, ProviderKind, ServerMessage,
        parse_local_tool_calls, resolve_run_provider,
    };
    use riga_kernel::events::RigaEvent;
    use tokio::sync::mpsc;

    #[test]
    fn protocol_accepts_provider_configuration_without_persisting_it() {
        let message: ClientMessage = serde_json::from_str(r#"{"type":"configure_provider","endpoint":"https://api.example/v1","api_key":"ephemeral","model":"opencode-go"}"#).unwrap();
        assert!(
            matches!(message, ClientMessage::ConfigureProvider(ProviderConfig { model, .. }) if model == "opencode-go")
        );
        let ready = serde_json::to_string(&ServerMessage::Event {
            envelope: super::envelope("run", "session", 1, RigaEvent::RunStarted),
        })
        .unwrap();
        assert!(!ready.contains("ephemeral"));
    }

    #[test]
    fn unknown_commands_are_rejected() {
        assert!(
            serde_json::from_str::<ClientMessage>(
                r#"{"type":"execute_shell","command":"rm -rf /"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn provider_configured_echoes_the_active_backend_kind() {
        // The client cannot infer the backend from a stored config, because a
        // local provider keeps the previous remote endpoint and model so a user
        // can switch back. `kind` is therefore part of the frame.
        let local = serde_json::to_string(&ServerMessage::ProviderConfigured {
            endpoint: "https://api.example/v1".into(),
            model: "opencode-go".into(),
            reasoning_effort: "low".into(),
            kind: ProviderKind::Local,
            api: ProviderApi::Chat,
            subagent_model: None,
        })
        .unwrap();
        assert!(local.contains(r#""kind":"local""#), "{local}");
        assert!(local.contains(r#""endpoint":"https://api.example/v1""#));
    }

    #[test]
    fn a_gpt5_model_on_a_compatible_endpoint_uses_chat_completions() {
        // The reported bug: the `gpt-5` name prefix alone selected the Responses
        // API, and an OpenAI-compatible gateway answered `/responses` with an
        // HTML 404. The shape is explicit now and defaults to the compatible
        // chat contract.
        let config: ProviderConfig = serde_json::from_str(
            r#"{"endpoint":"https://api.example/v1","api_key":"key","model":"gpt-5-nano"}"#,
        )
        .unwrap();
        assert_eq!(config.api, ProviderApi::Chat);
    }

    #[test]
    fn responses_api_is_opt_in_and_survives_a_round_trip() {
        let config: ProviderConfig = serde_json::from_str(
            r#"{"endpoint":"https://api.openai.com/v1","api_key":"key","model":"gpt-5-codex","api":"responses"}"#,
        )
        .unwrap();
        assert_eq!(config.api, ProviderApi::Responses);
    }

    #[test]
    fn completion_request_uses_strict_max_completion_tokens() {
        let body = super::completion_request_body_with_tools(
            "gpt-5-nano",
            &[serde_json::json!({"role": "user", "content": "hello"})],
            Vec::new(),
        );
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["model"], "gpt-5-nano");
    }

    #[test]
    fn coding_agent_prompt_requires_tools_for_implementation_requests() {
        let prompt = super::coding_agent_system_prompt();
        assert!(prompt.contains("use the available tools"));
        assert!(prompt.contains("Never claim a file or command succeeded"));
    }

    #[test]
    fn coding_agent_prompt_states_capabilities_and_agent_mentions() {
        // The model knows writes/shell pause for approval and that `@agent`
        // addresses a dispatchable subagent.
        let prompt = super::coding_agent_system_prompt();
        assert!(prompt.contains("pause for the user's approval"), "{prompt}");
        assert!(prompt.contains("@explore"), "{prompt}");
        assert!(prompt.contains("task"), "{prompt}");
    }

    #[test]
    fn trim_history_keeps_the_most_recent_turns_within_budget() {
        let turns: Vec<super::ConversationTurn> = (0..40)
            .map(|index| super::ConversationTurn {
                role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("turn-{index}"),
            })
            .collect();
        let trimmed = super::trim_history(&turns);
        assert_eq!(trimmed.len(), super::MAX_HISTORY_TURNS);
        assert_eq!(trimmed.last().unwrap().content, "turn-39");
        assert_eq!(trimmed.first().unwrap().content, "turn-24");
    }

    #[test]
    fn trim_history_drops_oldest_when_over_the_character_budget() {
        let long = "x".repeat(20_000);
        let turns = vec![
            super::ConversationTurn {
                role: "user".into(),
                content: long.clone(),
            },
            super::ConversationTurn {
                role: "assistant".into(),
                content: long.clone(),
            },
            super::ConversationTurn {
                role: "user".into(),
                content: "latest".into(),
            },
        ];
        let trimmed = super::trim_history(&turns);
        assert_eq!(trimmed.len(), 2, "the oldest turn should be dropped");
        assert_eq!(trimmed.last().unwrap().content, "latest");
    }

    #[test]
    fn provider_defaults_to_low_reasoning_effort_and_responses_tools_are_flat() {
        let config: ProviderConfig = serde_json::from_str(
            r#"{"endpoint":"https://api.example/v1","api_key":"key","model":"gpt-5-codex"}"#,
        )
        .unwrap();
        assert_eq!(config.reasoning_effort, "low");
        let tools = super::responses_tool_schemas(&[]);
        assert_eq!(tools[0]["type"], "function");
        assert!(tools[0].get("function").is_none());
        assert_eq!(tools[0]["name"], "read");
    }

    #[test]
    fn responses_text_extractor_handles_top_level_text_and_refusal() {
        assert_eq!(
            super::extract_response_text(&serde_json::json!({"output_text":"Example Domain"}))
                .unwrap(),
            "Example Domain"
        );
        let refusal = super::extract_response_text(&serde_json::json!({
            "status": "completed",
            "output": [{"type":"message", "content":[{"type":"refusal", "refusal":"not allowed"}]}]
        }))
        .unwrap_err();
        assert!(refusal.contains("provider refused the request: not allowed"));
    }

    #[test]
    fn task_agent_profiles_support_aliases_and_read_only_rules() {
        let profiles = crate::catalog::agent_profiles();
        assert_eq!(profiles.len(), 4);
        assert!(
            profiles
                .iter()
                .find(|profile| profile.name == "explore")
                .unwrap()
                .read_only
        );
        assert!(
            !profiles
                .iter()
                .find(|profile| profile.name == "build")
                .unwrap()
                .read_only
        );
        let dispatch = crate::catalog::execute_task(&serde_json::json!({
            "action": "dispatch",
            "agent": "executor",
            "prompt": "Create an Express health server"
        }))
        .unwrap();
        assert!(dispatch.contains("\"name\": \"build\""));
        assert!(dispatch.contains("Create an Express health server"));
    }

    #[test]
    fn a_loaded_local_model_serves_a_run_without_any_saved_provider() {
        // The reported bug: download a model, load it, send a message, and the
        // run was refused with provider_not_configured because nothing had been
        // saved through Settings yet. Loading a model is itself a provider
        // choice, so it must be enough.
        let resolved = resolve_run_provider(None, true).expect("a loaded model must serve the run");
        assert_eq!(resolved.kind, ProviderKind::Local);
        // A local run reads neither of these, and inheriting a stale remote
        // endpoint here would be misleading.
        assert!(resolved.endpoint.is_empty());
        assert!(resolved.api_key.is_empty());
        assert!(resolved.reasoning_effort == "low");
    }

    #[test]
    fn nothing_loaded_and_nothing_saved_still_reports_a_missing_provider() {
        let (code, message) = resolve_run_provider(None, false).expect_err("must refuse");
        assert_eq!(code, "provider_not_configured");
        // The remedy has to mention both options, since either one unblocks it.
        assert!(
            message.contains("local model") && message.contains("Settings"),
            "unhelpful message: {message}"
        );
    }

    #[test]
    fn a_stored_provider_wins_over_the_local_fallback() {
        // An explicit remote choice must not be silently replaced by whatever
        // model happens to be resident in memory.
        let stored = ProviderConfig {
            endpoint: "https://api.example/v1".into(),
            api_key: "k".into(),
            model: "opencode-go".into(),
            reasoning_effort: "high".into(),
            kind: ProviderKind::Remote,
            api: ProviderApi::Chat,
            subagent_model: None,
        };
        let resolved = resolve_run_provider(Some(&stored), true).expect("stored provider must win");
        assert_eq!(resolved.kind, ProviderKind::Remote);
        assert_eq!(resolved.model, "opencode-go");
        assert_eq!(resolved.reasoning_effort, "high");
    }

    #[test]
    fn a_stored_local_provider_is_passed_through_unchanged() {
        // Preserved so the caller's not-loaded check can produce the specific
        // "load a model" error instead of the generic one.
        let stored = ProviderConfig {
            endpoint: String::new(),
            api_key: String::new(),
            model: "local".into(),
            reasoning_effort: "medium".into(),
            kind: ProviderKind::Local,
            api: ProviderApi::Chat,
            subagent_model: None,
        };
        let resolved = resolve_run_provider(Some(&stored), false).expect("passed through");
        assert_eq!(resolved.kind, ProviderKind::Local);
        assert_eq!(resolved.reasoning_effort, "medium");
    }

    #[test]
    fn provider_kind_defaults_to_remote_for_older_clients() {
        // A client written before local models existed omits `kind` entirely; it
        // must keep working rather than deserialising into a local run.
        let message: ClientMessage = serde_json::from_str(
            r#"{"type":"configure_provider","endpoint":"https://api.example/v1","api_key":"k","model":"opencode-go"}"#,
        )
        .unwrap();
        let ClientMessage::ConfigureProvider(config) = message else {
            panic!("expected configure_provider");
        };
        assert_eq!(config.kind, ProviderKind::Remote);
        assert!(!config.is_local());
    }

    #[test]
    fn provider_kind_round_trips_local() {
        let config: ProviderConfig =
            serde_json::from_str(r#"{"endpoint":"","api_key":"","model":"local","kind":"local"}"#)
                .unwrap();
        assert!(config.is_local());
        // It must survive the round trip the secure store performs, since that
        // JSON is what a reconnecting client reads back.
        let stored = serde_json::to_string(&config).unwrap();
        assert!(stored.contains("\"kind\":\"local\""), "got {stored}");
        assert_eq!(
            serde_json::from_str::<ProviderConfig>(&stored)
                .unwrap()
                .kind,
            ProviderKind::Local
        );
    }

    #[test]
    fn a_single_tool_call_is_extracted_and_prose_is_kept() {
        let (prose, calls) = parse_local_tool_calls(
            "Let me look.\n<tool_call>{\"name\":\"read_file\",\"arguments\":{\"path\":\"a.txt\"}}</tool_call>",
        );
        assert_eq!(prose, "Let me look.");
        assert_eq!(
            calls,
            vec![ParsedToolCall {
                name: "read_file".into(),
                arguments: serde_json::json!({ "path": "a.txt" }),
            }]
        );
    }

    #[test]
    fn a_pure_tool_call_leaves_no_empty_prose() {
        let (prose, calls) = parse_local_tool_calls(
            "<tool_call>{\"name\":\"list_files\",\"arguments\":{}}</tool_call>",
        );
        assert_eq!(prose, "", "the protocol block must not leak into the reply");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_files");
    }

    #[test]
    fn several_tool_calls_in_one_reply_are_all_returned_in_order() {
        let (prose, calls) = parse_local_tool_calls(
            "<tool_call>{\"name\":\"a\",\"arguments\":{}}</tool_call> then <tool_call>{\"name\":\"b\",\"arguments\":{}}</tool_call>",
        );
        assert_eq!(prose, "then");
        assert_eq!(
            calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn arguments_shapes_and_quoting_slips_are_tolerated() {
        // Small models emit the arguments key inconsistently, and sometimes wrap
        // the object in a JSON string. Both must still reach the tool executor.
        // Each case states its expected arguments explicitly: a substring
        // heuristic would silently mis-read the escaped-string case.
        let one = serde_json::json!({ "x": 1 });
        let cases: [(&str, serde_json::Value); 5] = [
            (r#"{"name":"t","arguments":{"x":1}}"#, one.clone()),
            (r#"{"name":"t","parameters":{"x":1}}"#, one.clone()),
            (r#"{"name":"t","input":{"x":1}}"#, one.clone()),
            (r#"{"name":"t","arguments":"{\"x\":1}"}"#, one.clone()),
            (r#"{"name":"t"}"#, serde_json::json!({})),
        ];
        for (raw, expected) in cases {
            let (prose, calls) = parse_local_tool_calls(&format!("<tool_call>{raw}</tool_call>"));
            assert_eq!(calls.len(), 1, "no call parsed from {raw}");
            assert_eq!(calls[0].name, "t", "for {raw}");
            assert_eq!(calls[0].arguments, expected, "for {raw}");
            assert!(prose.is_empty(), "protocol block leaked for {raw}");
        }
    }

    #[test]
    fn malformed_blocks_do_not_destroy_the_reply() {
        // Unparseable or unterminated JSON must not cost the user their answer.
        for reply in [
            "Here is the answer.\n<tool_call>{not json}</tool_call>",
            "Here is the answer.\n<tool_call>{\"name\":\"a\"",
            "no protocol here at all",
        ] {
            let (prose, calls) = parse_local_tool_calls(reply);
            assert!(calls.is_empty(), "unexpected call in {reply:?}");
            assert!(
                prose.contains("Here is the answer.") || prose == "no protocol here at all",
                "prose lost for {reply:?}, got {prose:?}"
            );
        }
    }

    #[test]
    fn a_block_with_no_name_is_rejected_rather_than_executed() {
        // An unnamed call cannot be dispatched, and guessing a tool would be
        // worse than ignoring it.
        let (_, calls) = parse_local_tool_calls("<tool_call>{\"arguments\":{}}</tool_call>");
        assert!(calls.is_empty());
        let (_, calls) = parse_local_tool_calls("<tool_call>{\"name\":\"  \"}</tool_call>");
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn update_plan_emits_a_plan_frame_and_confirms() {
        let (sender, mut receiver) = mpsc::channel(4);
        let stream = super::ToolOutputStream {
            call_id: "call-1".into(),
            trace_sender: sender,
        };
        let result = super::execute_tool(
            std::path::Path::new("."),
            &crate::mcp::McpRuntime::new(),
            "update_plan",
            serde_json::json!({
                "title": "Build a service",
                "steps": [{"id": "probe", "label": "add /probe"}],
                "active_index": 0
            }),
            Some(stream),
        )
        .await
        .expect("update_plan executes");
        assert!(result.contains("Plan updated"), "{result}");
        match receiver.recv().await.expect("a frame was emitted") {
            super::ToolTraceEvent::Ui(RigaEvent::PlanUpdated { plan }) => {
                assert_eq!(plan.steps.len(), 1);
                assert_eq!(plan.title, "Build a service");
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[tokio::test]
    async fn update_todos_reports_the_element_ratio() {
        let (sender, mut receiver) = mpsc::channel(4);
        let stream = super::ToolOutputStream {
            call_id: "call-1".into(),
            trace_sender: sender,
        };
        let result = super::execute_tool(
            std::path::Path::new("."),
            &crate::mcp::McpRuntime::new(),
            "update_todos",
            serde_json::json!({
                "items": [
                    {"id": "1", "text": "done", "status": "done"},
                    {"id": "2", "text": "failed", "status": "failed"},
                    {"id": "3", "text": "dropped", "status": "cancelled"}
                ]
            }),
            Some(stream),
        )
        .await
        .expect("update_todos executes");
        assert!(result.contains("1/2"), "{result}");
        assert!(matches!(
            receiver.recv().await.expect("a frame was emitted"),
            super::ToolTraceEvent::Ui(RigaEvent::TodoUpdated { .. })
        ));
    }

    #[tokio::test]
    async fn malformed_plan_arguments_are_reported_not_ignored() {
        let error = super::execute_tool(
            std::path::Path::new("."),
            &crate::mcp::McpRuntime::new(),
            "update_plan",
            serde_json::json!({"title": "x"}),
            None,
        )
        .await
        .expect_err("missing steps must fail");
        assert!(
            error.contains("update_plan arguments are invalid"),
            "{error}"
        );
    }

    #[test]
    fn subagent_dispatch_is_detected_from_the_task_arguments() {
        assert!(super::is_subagent_dispatch(
            &serde_json::json!({"action": "dispatch", "agent": "explore", "prompt": "find it"})
        ));
        assert!(super::is_subagent_dispatch(
            &serde_json::json!({"action": "agent", "agent": "build", "prompt": "do it"})
        ));
        // Listing agents, or a dispatch with no agent, is not a subagent run.
        assert!(!super::is_subagent_dispatch(
            &serde_json::json!({"action": "agents"})
        ));
        assert!(!super::is_subagent_dispatch(
            &serde_json::json!({"action": "dispatch", "prompt": "no agent"})
        ));
    }

    #[test]
    fn leading_agent_mentions_are_resolved_to_the_canonical_profile() {
        assert_eq!(
            super::leading_agent_mention("@explore find the entry point"),
            Some(("explore".to_owned(), "find the entry point".to_owned()))
        );
        assert_eq!(
            super::leading_agent_mention("  @build add a route").map(|(agent, _)| agent),
            Some("build".to_owned())
        );
        // An alias resolves to the profile's canonical name.
        assert_eq!(
            super::leading_agent_mention("@scout look around").map(|(agent, _)| agent),
            Some("explore".to_owned())
        );
        // A mention that is not at the start, has no task, or names no known
        // agent is left to the orchestrator.
        assert!(super::leading_agent_mention("email @explore later").is_none());
        assert!(super::leading_agent_mention("@explore").is_none());
        assert!(super::leading_agent_mention("@unknown do a thing").is_none());
    }

    #[test]
    fn read_only_profiles_cannot_write_or_dispatch() {
        let explore = crate::catalog::find_agent_profile("explore").expect("explore profile");
        let allowed = super::allowed_tools_for(&explore);
        for forbidden in ["write", "bash", "task"] {
            assert!(
                !allowed.iter().any(|tool| tool == forbidden),
                "explore must not be allowed `{forbidden}`: {allowed:?}"
            );
        }
        assert!(allowed.iter().any(|tool| tool == "read"));
        assert!(allowed.iter().any(|tool| tool == "glob"));
    }

    #[test]
    fn build_profile_may_write_and_dispatch() {
        let build = crate::catalog::find_agent_profile("build").expect("build profile");
        let allowed = super::allowed_tools_for(&build);
        for expected in ["read", "write", "bash", "task"] {
            assert!(
                allowed.iter().any(|tool| tool == expected),
                "build should be allowed `{expected}`: {allowed:?}"
            );
        }
    }

    #[test]
    fn read_only_subagents_use_the_subagent_model_when_configured() {
        let mut config = ProviderConfig {
            endpoint: "https://api.example/v1".into(),
            api_key: "k".into(),
            model: "main-model".into(),
            reasoning_effort: "low".into(),
            kind: ProviderKind::Remote,
            api: ProviderApi::Chat,
            subagent_model: Some("cheap-model".into()),
        };
        let explore = crate::catalog::find_agent_profile("explore").unwrap();
        let build = crate::catalog::find_agent_profile("build").unwrap();
        assert_eq!(super::subagent_model_for(&config, &explore), "cheap-model");
        assert_eq!(super::subagent_model_for(&config, &build), "main-model");

        // Unset or blank falls back to the main model.
        config.subagent_model = None;
        assert_eq!(super::subagent_model_for(&config, &explore), "main-model");
        config.subagent_model = Some("   ".into());
        assert_eq!(super::subagent_model_for(&config, &explore), "main-model");
    }

    #[test]
    fn subagent_prompt_carries_the_profile_rules_and_sections() {
        let review = crate::catalog::find_agent_profile("review").expect("review profile");
        let prompt = super::subagent_system_prompt(&review);
        assert!(prompt.contains("`review` subagent"), "{prompt}");
        assert!(prompt.contains("Never modify files"), "{prompt}");
        assert!(prompt.contains("Findings"), "{prompt}");
        assert!(prompt.contains("do not ask the user"), "{prompt}");
    }

    #[tokio::test]
    async fn a_gated_tool_waits_for_approval_then_honours_always() {
        let broker = super::ApprovalBroker::default();
        let (sender, mut receiver) = mpsc::channel(4);
        let broker_for_task = broker.clone();
        let sender_for_task = sender.clone();
        let handle = tokio::spawn(async move {
            super::authorize_tool(
                &broker_for_task,
                &sender_for_task,
                "call-1",
                "write",
                &serde_json::json!({"path": "a.txt", "content": "x"}),
            )
            .await
        });

        let approval_id = match receiver.recv().await.expect("approval requested") {
            super::ToolTraceEvent::Ui(RigaEvent::ApprovalRequested {
                approval_id, tool, ..
            }) => {
                assert_eq!(tool, "write");
                approval_id
            }
            other => panic!("unexpected frame: {other:?}"),
        };
        broker
            .resolve(
                &approval_id,
                super::ApprovalReply {
                    approved: true,
                    always: true,
                },
            )
            .await;
        let _ = receiver.recv().await; // ApprovalResolved
        handle.await.unwrap().expect("authorized once");

        // "Always" means the next call of the same tool does not ask again.
        super::authorize_tool(
            &broker,
            &sender,
            "call-2",
            "write",
            &serde_json::json!({"path": "b.txt", "content": "y"}),
        )
        .await
        .expect("always allowed");
        assert!(
            receiver.try_recv().is_err(),
            "no second approval should be requested"
        );
    }

    #[tokio::test]
    async fn a_denied_gated_tool_reports_the_denial() {
        let broker = super::ApprovalBroker::default();
        let (sender, mut receiver) = mpsc::channel(4);
        let broker_for_task = broker.clone();
        let sender_for_task = sender.clone();
        let handle = tokio::spawn(async move {
            super::authorize_tool(
                &broker_for_task,
                &sender_for_task,
                "call-1",
                "bash",
                &serde_json::json!({"command": "rm -rf /"}),
            )
            .await
        });
        let approval_id = match receiver.recv().await.expect("approval requested") {
            super::ToolTraceEvent::Ui(RigaEvent::ApprovalRequested { approval_id, .. }) => {
                approval_id
            }
            other => panic!("unexpected frame: {other:?}"),
        };
        broker
            .resolve(
                &approval_id,
                super::ApprovalReply {
                    approved: false,
                    always: false,
                },
            )
            .await;
        let error = handle.await.unwrap().expect_err("denied");
        assert!(error.contains("denied"), "{error}");
    }

    #[tokio::test]
    async fn a_read_only_tool_is_allowed_without_asking() {
        let broker = super::ApprovalBroker::default();
        let (sender, mut receiver) = mpsc::channel(4);
        super::authorize_tool(
            &broker,
            &sender,
            "call-1",
            "read",
            &serde_json::json!({"path": "a.txt"}),
        )
        .await
        .expect("read is allowed");
        assert!(
            receiver.try_recv().is_err(),
            "read must not request approval"
        );
    }
}
