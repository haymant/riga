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
pub(crate) struct ApprovalReply {
    pub(crate) approved: bool,
    /// Approve this tool for the rest of the session, not just this call.
    pub(crate) always: bool,
}

#[derive(Clone, Default)]
struct RunEvidence {
    /// Set once any workspace-write or process-execution tool succeeds, in this
    /// loop or any subagent's, so a child's work counts toward the parent.
    mutated: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set once any token was streamed to the client, so the final `RunCompleted`
    /// path does not resend the whole reply as one delta on top of it.
    streamed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set when the run is cancelled. Threaded through the whole run (including
    /// subagents) so a cancellation reaches a model that is mid-decode, not just
    /// the loop that spawned it. Carrying it here rather than as another
    /// parameter keeps the dozens of loop call sites unchanged.
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    graph: std::sync::Arc<tokio::sync::Mutex<GraphRuntime>>,
    /// Run-scoped local-model budget, adjusted by `set_model_budget`.
    budget: std::sync::Arc<tokio::sync::Mutex<crate::local_model::LocalBudget>>,
    /// Tools granted to a profile for this run, on top of its defaults, by
    /// `grant_tools` after the user approves.
    tool_grants: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, Vec<String>>>>,
}

#[derive(Default)]
struct GraphRuntime {
    graph: Option<riga_kernel::task::Graph>,
    states: std::collections::HashMap<String, riga_kernel::task::TaskState>,
}

impl RunEvidence {
    fn with_cancel(cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self {
            cancelled,
            ..Self::default()
        }
    }

    /// Record a tool result. Only a *successful* mutating call counts; a failed
    /// write or a denied approval is not evidence of work.
    fn record(&self, tool: &str, ok: bool) {
        if ok && is_mutating_tool(tool) {
            // SeqCst: the run's task can migrate between worker threads at await
            // points, and a missed store would make the verification guard fire
            // on work the run really did.
            self.mutated
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn has_work(&self) -> bool {
        self.mutated.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn streamed(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.streamed.clone()
    }

    fn mark_streamed(&self) {
        self.streamed
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn was_streamed(&self) -> bool {
        self.streamed.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn cancel_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.cancelled.clone()
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn graph(&self) -> std::sync::Arc<tokio::sync::Mutex<GraphRuntime>> {
        self.graph.clone()
    }

    fn budget(&self) -> std::sync::Arc<tokio::sync::Mutex<crate::local_model::LocalBudget>> {
        self.budget.clone()
    }

    fn tool_grants(
        &self,
    ) -> std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, Vec<String>>>> {
        self.tool_grants.clone()
    }
}

/// How many run events a subscriber may fall behind before it is told it lagged
/// and catches up from the journal instead.
pub(crate) const RUN_EVENT_BUFFER: usize = 1024;

/// A run that is currently executing on the server.
#[derive(Clone)]
pub(crate) struct RunHandle {
    pub(crate) session_id: String,
    /// Live events. A socket subscribes to follow the run; the run outlives any
    /// individual subscriber.
    pub(crate) events: tokio::sync::broadcast::Sender<RigaEventEnvelope>,
    /// Set on `CancelRun`; the event loops poll it, including mid-decode.
    pub(crate) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether this run uses the (single) local engine, so a second local run
    /// can be refused with a clear message instead of blocking on the mutex.
    pub(crate) local: bool,
}

/// The runs in flight, keyed by run id. Lives in server state so every socket
/// sees the same runs.
pub(crate) type RunRegistry =
    std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, RunHandle>>>;

#[derive(Debug, Serialize)]
pub struct ActiveRun {
    pub(crate) run_id: String,
    pub(crate) session_id: String,
    pub(crate) local: bool,
}

/// A frame that is streamed but not worth persisting.
fn is_ephemeral(event: &RigaEvent) -> bool {
    matches!(
        event,
        RigaEvent::TextDelta { .. }
            | RigaEvent::ReasoningDelta { .. }
            | RigaEvent::ToolOutputDelta { .. }
    )
}

fn is_terminal(event: &RigaEvent) -> bool {
    matches!(
        event,
        RigaEvent::RunCompleted { .. } | RigaEvent::RunFailed { .. }
    )
}

/// Forwards local-model tokens to the UI as they are decoded, without leaking
/// the `<tool_call>` block into the visible transcript.
///
/// A turn is either prose or a single tool call, and that cannot be known until
/// enough of the turn has arrived. The opening bytes are therefore held back and
/// then either streamed once the marker is ruled out, or dropped once it is
/// recognised. Sends block, because this runs on the generation thread and the
/// read loop drains the channel.
struct LocalDeltaStream {
    sender: mpsc::Sender<ToolTraceEvent>,
    pending: String,
    /// `None` until the turn's shape is known; `Some(true)` prose, `Some(false)`
    /// a tool call.
    decided: Option<bool>,
    /// True while inside a ` thinking…` block, whose text streams as reasoning.
    in_reasoning: bool,
    /// Set by `generate` once the prompt is formatted. Read on the first token:
    /// a template that opened the reasoning block means the output starts with
    /// the reasoning and carries no opening tag of its own.
    reasoning_expected: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reasoning_checked: bool,
    streamed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl LocalDeltaStream {
    const MARKER: &'static str = "<tool_call>";
    /// Qwen3-style reasoning block; streamed as reasoning, not reply text.
    // `<` and `>` written as escapes: the literal tag is stripped by tooling.
    const REASONING_OPEN: &'static str = "\u{3c}think\u{3e}";
    const REASONING_CLOSE: &'static str = "\u{3c}/think\u{3e}";

    fn new(
        sender: mpsc::Sender<ToolTraceEvent>,
        streamed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        reasoning_expected: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            sender,
            pending: String::new(),
            decided: None,
            in_reasoning: false,
            reasoning_expected,
            reasoning_checked: false,
            streamed,
        }
    }

    fn push(&mut self, delta: &str) {
        if !self.reasoning_checked {
            self.reasoning_checked = true;
            if self
                .reasoning_expected
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                self.in_reasoning = true;
            }
        }
        if self.decided == Some(true) {
            self.emit_text(delta);
            return;
        }
        if self.decided == Some(false) {
            return;
        }
        self.pending.push_str(delta);
        loop {
            if self.in_reasoning {
                // A reasoning model may repeat the opening tag the template
                // already added; drop it from the reasoning text.
                let start = self.pending.len() - self.pending.trim_start().len();
                if self.pending[start..].starts_with(Self::REASONING_OPEN) {
                    self.pending = self.pending[start + Self::REASONING_OPEN.len()..]
                        .trim_start()
                        .to_owned();
                }
                if let Some(end) = self.pending.find(Self::REASONING_CLOSE) {
                    let reasoning = self.pending[..end].to_owned();
                    self.emit_reasoning(&reasoning);
                    self.pending = self.pending[end + Self::REASONING_CLOSE.len()..].to_owned();
                    self.in_reasoning = false;
                } else {
                    // Stream reasoning as it arrives, holding back a short tail that
                    // may be the start of the closing tag. Snap to a char boundary.
                    let keep = Self::REASONING_CLOSE.len();
                    if self.pending.len() > keep {
                        let split = (0..=self.pending.len() - keep)
                            .rev()
                            .find(|index| self.pending.is_char_boundary(*index))
                            .unwrap_or(0);
                        let ready = self.pending[..split].to_owned();
                        self.emit_reasoning(&ready);
                        self.pending = self.pending[split..].to_owned();
                    }
                    return;
                }
            } else {
                let start = self.pending.len() - self.pending.trim_start().len();
                if self.pending[start..].starts_with(Self::REASONING_OPEN) {
                    self.pending = self.pending[start + Self::REASONING_OPEN.len()..].to_owned();
                    self.in_reasoning = true;
                    continue;
                }
                let trimmed = self.pending.trim_start();
                if trimmed.starts_with(Self::MARKER) {
                    self.decided = Some(false);
                    self.pending.clear();
                    return;
                }
                if trimmed.len() >= Self::MARKER.len() || !Self::MARKER.starts_with(trimmed) {
                    self.decided = Some(true);
                    let buffered = std::mem::take(&mut self.pending);
                    self.emit_text(&buffered);
                    return;
                }
                return;
            }
        }
    }

    /// Flush whatever is buffered when generation ends.
    fn finish(&mut self) {
        if self.in_reasoning {
            let rest = std::mem::take(&mut self.pending);
            self.emit_reasoning(&rest);
            self.in_reasoning = false;
        }
        if self.decided.is_none() {
            self.decided = Some(true);
            let buffered = std::mem::take(&mut self.pending);
            self.emit_text(&buffered);
        }
    }

    fn emit_text(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.streamed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.sender.blocking_send(ToolTraceEvent::Ui(
            riga_kernel::events::RigaEvent::TextDelta {
                delta: delta.to_owned(),
            },
        ));
    }

    fn emit_reasoning(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let _ = self.sender.blocking_send(ToolTraceEvent::Ui(
            riga_kernel::events::RigaEvent::ReasoningDelta {
                delta: delta.to_owned(),
            },
        ));
    }
}

/// Whether a successful call to this tool changes the workspace or runs a
/// process. A read, glob, grep, or web fetch proves nothing was built.
fn is_mutating_tool(tool: &str) -> bool {
    matches!(
        riga_kernel::policy::ToolRisk::for_tool(tool),
        riga_kernel::policy::ToolRisk::WorkspaceWrite
            | riga_kernel::policy::ToolRisk::ProcessExecution
    )
}

/// Completion claims that a run must be able to back with a tool result.
///
/// Deliberately narrow. Bare "created"/"implemented" appear in ordinary
/// explanation, so only first-person and passive completion phrasing counts;
/// the guard must not fire on a run that was asked a question and answered it.
const WORK_CLAIMS: [&str; 21] = [
    "has been created",
    "have been created",
    "was created",
    "were created",
    "i created",
    "i've created",
    "i have created",
    "i wrote",
    "i've written",
    "i have written",
    "i added",
    "i've added",
    "i implemented",
    "i installed",
    "i started",
    "i ran ",
    "successfully created",
    "successfully installed",
    "the server is running",
    "now listening on",
    "files changed",
];

/// True when the final prose claims work that only a tool result can prove.
fn claims_work(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    WORK_CLAIMS.iter().any(|claim| lower.contains(claim))
}

/// How many times a run is told to actually do the work before its claim is
/// annotated instead. Two gives the model a chance to recover from a mistaken
/// summary without letting it loop on one.
const MAX_CLAIM_NUDGES: usize = 2;

const CLAIM_NUDGE: &str = "You reported changes, but no successful file write or command was recorded in this run. \
Use the `write` or `bash` tool to actually make the change now, or correct your answer to state that nothing was changed. \
Do not repeat a claim you cannot back with a tool result.";

/// Resolves tool approvals for one WebSocket session.
///
/// A run suspends inside `request_approval`; the socket's read loop resolves it
/// when the `Approval` frame arrives. `always_allowed` persists across runs in
/// the same session so "always allow" means exactly that.
#[derive(Clone, Default)]
pub(crate) struct ApprovalBroker {
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
    pub(crate) async fn resolve(&self, approval_id: &str, reply: ApprovalReply) -> bool {
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
    ListActiveRuns,
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
    ActiveRuns {
        runs: Vec<ActiveRun>,
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

/// Stream one run's events to a socket until the run ends or the socket drops.
///
/// When `replay` is set, the journal is sent from `after_sequence` first; then
/// the live broadcast is followed, deduplicating by sequence. `replay` is for a
/// reconnecting client catching up on a run it has partly seen. A fresh
/// `start_run` passes `false`: its receiver was subscribed before the run was
/// spawned, so every frame is already buffered, and replaying the journal (which
/// omits ephemeral token frames) would only advance the cursor past them.
///
/// Returning does **not** stop the run: that is what lets a locked phone keep
/// thinking and catch up on unlock.
#[allow(clippy::too_many_arguments)]
async fn forward_run<S>(
    sender: &mut S,
    receiver: &mut futures_util::stream::SplitStream<WebSocket>,
    mut upstream: tokio::sync::broadcast::Receiver<RigaEventEnvelope>,
    run_id: &str,
    after_sequence: u64,
    replay: bool,
    broker: &ApprovalBroker,
    runs: &RunRegistry,
) where
    S: SinkExt<Message> + Unpin,
{
    let mut last = after_sequence;
    // Catch up on anything already journaled. The receiver was subscribed before
    // this read, so live frames published meanwhile are buffered, not lost; the
    // sequence guard below drops any that overlap the replay.
    if replay
        && let Ok(journal) = riga_kernel::persistence::EventJournal::open(run_journal_path(run_id))
    {
        for replay in journal.after_sequence(after_sequence) {
            last = last.max(replay.sequence);
            if send(sender, ServerMessage::Event { envelope: replay })
                .await
                .is_err()
            {
                return;
            }
        }
    }
    loop {
        tokio::select! {
            event = upstream.recv() => {
                match event {
                    Ok(envelope) => {
                        if envelope.sequence <= last {
                            continue;
                        }
                        last = envelope.sequence;
                        let terminal = is_terminal(&envelope.event);
                        if send(sender, ServerMessage::Event { envelope }).await.is_err() {
                            return;
                        }
                        if terminal {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // A slow socket missed frames; the journal is complete,
                        // so replay from where the client was.
                        if let Ok(journal) = riga_kernel::persistence::EventJournal::open(run_journal_path(run_id)) {
                            for replay in journal.after_sequence(last) {
                                last = replay.sequence;
                                if send(sender, ServerMessage::Event { envelope: replay }).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
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
                            Ok(ClientMessage::ListActiveRuns) => {
                                let runs = runs
                                    .lock()
                                    .await
                                    .iter()
                                    .map(|(run_id, handle)| ActiveRun {
                                        run_id: run_id.clone(),
                                        session_id: handle.session_id.clone(),
                                        local: handle.local,
                                    })
                                    .collect();
                                let _ = send(sender, ServerMessage::ActiveRuns { runs }).await;
                            }
                            Ok(ClientMessage::CancelRun { run_id }) => {
                                if let Some(handle) = runs.lock().await.get(&run_id) {
                                    handle
                                        .cancel
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                                let _ = send(sender, ServerMessage::RunCancelled { run_id }).await;
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sender.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => return,
                    _ => {}
                }
            }
        }
    }
}

/// Start a run without a WebSocket. The Tauri adapter subscribes to the same
/// broadcast channel used by the HTTP adapter, so provider execution, tools,
/// approvals, persistence, cancellation, and event ordering remain owned here.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_ipc_run(
    workspace_root: PathBuf,
    mcp_runtime: McpRuntime,
    local_models: std::sync::Arc<crate::local_model::LocalModelRuntime>,
    transcripts: std::sync::Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, Vec<ConversationTurn>>>,
    >,
    secure_store: Option<std::sync::Arc<crate::secure_store::SecureStore>>,
    broker: ApprovalBroker,
    runs: RunRegistry,
    run_id: String,
    session_id: String,
    prompt: String,
) -> Result<tokio::sync::broadcast::Receiver<RigaEventEnvelope>, String> {
    let provider: Option<ProviderConfig> = crate::secure_store::load_json("provider")
        .await
        .map_err(|error| error.to_string())?;
    let config = resolve_run_provider(provider.as_ref(), local_models.loaded_file_name().is_some())
        .map_err(|(_, message)| message.to_owned())?;
    if config.is_local() && local_models.loaded_file_name().is_none() {
        return Err("Load a local model in the model manager before starting a run.".into());
    }
    let history = {
        let guard = transcripts.read().await;
        trim_history(guard.get(&session_id).map(Vec::as_slice).unwrap_or(&[]))
    };
    let run_root = crate::workspace::ensure_worktree(&workspace_root, &session_id).await?;
    let (events, cancel) = {
        let mut guard = runs.lock().await;
        if guard.contains_key(&run_id) {
            return Err("That run is already in progress.".into());
        }
        if config.is_local() && guard.values().any(|handle| handle.local) {
            return Err("A local model run is already in progress.".into());
        }
        let (events, _) = tokio::sync::broadcast::channel::<RigaEventEnvelope>(RUN_EVENT_BUFFER);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        guard.insert(
            run_id.clone(),
            RunHandle {
                session_id: session_id.clone(),
                events: events.clone(),
                cancel: cancel.clone(),
                local: config.is_local(),
            },
        );
        (events, cancel)
    };
    let receiver = events.subscribe();
    let run_id_task = run_id.clone();
    let session_id_task = session_id.clone();
    let prompt_task = prompt.clone();
    let config_task = config.clone();
    let mcp_task = mcp_runtime.clone();
    let local_task = local_models.clone();
    let history_task = history.clone();
    let broker_task = broker.clone();
    let transcripts_task = transcripts.clone();
    let store_task = secure_store.clone();
    let runs_task = runs.clone();
    let events_task = events.clone();
    tokio::spawn(async move {
        let evidence = RunEvidence::with_cancel(cancel);
        let mut channel = RunChannel::new(&run_id_task, &session_id_task, events_task);
        let output = run_provider(
            &mut channel,
            &broker_task,
            &config_task,
            &run_root,
            &prompt_task,
            &mcp_task,
            &local_task,
            &history_task,
            &evidence,
        )
        .await;
        let mut turns = vec![ConversationTurn {
            role: "user".into(),
            content: prompt_task,
        }];
        if let Some(output) = output {
            turns.push(ConversationTurn {
                role: "assistant".into(),
                content: output,
            });
        }
        append_turns(&transcripts_task, &store_task, &session_id_task, &turns).await;
        runs_task.lock().await.remove(&run_id_task);
    });
    Ok(receiver)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn upgrade(
    socket: WebSocket,
    workspace_root: PathBuf,
    mcp_runtime: McpRuntime,
    local_models: std::sync::Arc<crate::local_model::LocalModelRuntime>,
    transcripts: std::sync::Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, Vec<ConversationTurn>>>,
    >,
    secure_store: Option<std::sync::Arc<crate::secure_store::SecureStore>>,
    broker: ApprovalBroker,
    runs: RunRegistry,
) {
    let (mut sender, mut receiver) = socket.split();
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
                    Ok(ClientMessage::ListActiveRuns) => {
                        let runs = runs
                            .lock()
                            .await
                            .iter()
                            .map(|(run_id, handle)| ActiveRun {
                                run_id: run_id.clone(),
                                session_id: handle.session_id.clone(),
                                local: handle.local,
                            })
                            .collect();
                        let _ = send(&mut sender, ServerMessage::ActiveRuns { runs }).await;
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
                        if let Some(handle) = runs.lock().await.get(&run_id) {
                            handle
                                .cancel
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        let _ = send(&mut sender, ServerMessage::RunCancelled { run_id }).await;
                    }
                    Ok(ClientMessage::ResumeRun {
                        run_id,
                        after_sequence,
                    }) => {
                        // A run that is still in flight is followed live after the
                        // journal replay, so a reconnect sees the rest of it. A
                        // finished run has only the journal, replayed from the
                        // client's cursor without re-running the agent.
                        let upstream = runs
                            .lock()
                            .await
                            .get(&run_id)
                            .map(|handle| handle.events.subscribe());
                        match upstream {
                            Some(upstream) => {
                                forward_run(
                                    &mut sender,
                                    &mut receiver,
                                    upstream,
                                    &run_id,
                                    after_sequence,
                                    true,
                                    &broker,
                                    &runs,
                                )
                                .await;
                            }
                            None => {
                                match riga_kernel::persistence::EventJournal::open(
                                    run_journal_path(&run_id),
                                ) {
                                    Ok(journal) => {
                                        for replay in journal.after_sequence(after_sequence) {
                                            if send(
                                                &mut sender,
                                                ServerMessage::Event { envelope: replay },
                                            )
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
                        // Each session edits its own worktree on a
                        // `riga/<session>` branch forked from HEAD, so a new app
                        // cannot collide with an existing one and the base
                        // checkout is untouched.
                        let run_root =
                            match crate::workspace::ensure_worktree(&workspace_root, &session_id)
                                .await
                            {
                                Ok(root) => root,
                                Err(error) => {
                                    let _ = send(
                                        &mut sender,
                                        ServerMessage::Error {
                                            code: "workspace_unavailable".into(),
                                            message: error,
                                        },
                                    )
                                    .await;
                                    continue;
                                }
                            };
                        // Register the run before spawning so a `resume_run` that
                        // races the first frame still finds it, and refuse a
                        // duplicate id or a second local run (there is one engine,
                        // so the second would only block on its mutex).
                        let (events, cancel) = {
                            let mut guard = runs.lock().await;
                            if guard.contains_key(&run_id) {
                                drop(guard);
                                let _ = send(
                                    &mut sender,
                                    ServerMessage::Error {
                                        code: "run_already_active".into(),
                                        message: "That run is already in progress.".into(),
                                    },
                                )
                                .await;
                                continue;
                            }
                            if config.is_local() && guard.values().any(|handle| handle.local) {
                                drop(guard);
                                let _ = send(
                                    &mut sender,
                                    ServerMessage::Error {
                                        code: "local_run_in_progress".into(),
                                        message: "A local model run is already in progress. Wait for it or stop it before starting another.".into(),
                                    },
                                )
                                .await;
                                continue;
                            }
                            let (events, _) = tokio::sync::broadcast::channel::<RigaEventEnvelope>(
                                RUN_EVENT_BUFFER,
                            );
                            let cancel =
                                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                            guard.insert(
                                run_id.clone(),
                                RunHandle {
                                    session_id: session_id.clone(),
                                    events: events.clone(),
                                    cancel: cancel.clone(),
                                    local: config.is_local(),
                                },
                            );
                            (events, cancel)
                        };
                        // Subscribe before spawning so the run's opening frames are
                        // buffered for this socket rather than lost.
                        let upstream = events.subscribe();
                        {
                            let run_id_task = run_id.clone();
                            let session_id_task = session_id.clone();
                            let prompt_task = prompt.clone();
                            let config_task = config.clone();
                            let run_root_task = run_root.clone();
                            let mcp_task = mcp_runtime.clone();
                            let local_task = local_models.clone();
                            let history_task = history.clone();
                            let broker_task = broker.clone();
                            let transcripts_task = transcripts.clone();
                            let store_task = secure_store.clone();
                            let runs_task = runs.clone();
                            let events_task = events.clone();
                            tokio::spawn(async move {
                                let evidence = RunEvidence::with_cancel(cancel);
                                let mut channel =
                                    RunChannel::new(&run_id_task, &session_id_task, events_task);
                                let output = run_provider(
                                    &mut channel,
                                    &broker_task,
                                    &config_task,
                                    &run_root_task,
                                    &prompt_task,
                                    &mcp_task,
                                    &local_task,
                                    &history_task,
                                    &evidence,
                                )
                                .await;
                                let mut turns = vec![ConversationTurn {
                                    role: "user".into(),
                                    content: prompt_task,
                                }];
                                if let Some(output) = output {
                                    turns.push(ConversationTurn {
                                        role: "assistant".into(),
                                        content: output,
                                    });
                                }
                                append_turns(
                                    &transcripts_task,
                                    &store_task,
                                    &session_id_task,
                                    &turns,
                                )
                                .await;
                                runs_task.lock().await.remove(&run_id_task);
                            });
                        }
                        // Follow the run from the start out of the subscription
                        // buffer (it was taken before the run was spawned, so it
                        // holds every frame, token deltas included). Replaying the
                        // journal here would advance the cursor past those deltas.
                        forward_run(
                            &mut sender,
                            &mut receiver,
                            upstream,
                            &run_id,
                            0,
                            false,
                            &broker,
                            &runs,
                        )
                        .await;
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
    graph: std::sync::Arc<tokio::sync::Mutex<GraphRuntime>>,
    budget: std::sync::Arc<tokio::sync::Mutex<crate::local_model::LocalBudget>>,
    tool_grants: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, Vec<String>>>>,
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
                path_arg(&input)
                    .ok_or_else(|| argument_error("read", "{\"path\": \"<file>\"}", &input))?,
            )
            .await
        }
        "write" => {
            crate::catalog::execute_write(
                workspace_root,
                path_arg(&input).ok_or_else(|| {
                    argument_error(
                        "write",
                        "{\"path\": \"<file>\", \"content\": \"<text>\"} (one file per call)",
                        &input,
                    )
                })?,
                content_arg(&input).ok_or_else(|| {
                    argument_error(
                        "write",
                        "{\"path\": \"<file>\", \"content\": \"<text>\"} (one file per call)",
                        &input,
                    )
                })?,
            )
            .await
        }
        "bash" | "shell" => {
            let command = input
                .get("command")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    argument_error("bash", "{\"command\": \"<one shell command>\"}", &input)
                })?;
            if let Some(message) = install_preflight(workspace_root, command) {
                return Err(message);
            }
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
        "find_symbol" | "find_callers" | "find_references" | "find_tests" => {
            let query = input
                .get("symbol")
                .or_else(|| input.get("query"))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| argument_error(name, "{\"symbol\": \"<name>\"}", &input))?;
            crate::catalog::execute_repository_query(workspace_root, name, query).await
        }
        "add_evidence" => {
            let evidence = riga_kernel::events::EvidenceNode {
                id: input
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("evidence-1")
                    .into(),
                claim: input
                    .get("claim")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("add_evidence requires claim")?
                    .into(),
                source_ref: input
                    .get("source_ref")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("add_evidence requires source_ref")?
                    .into(),
                confidence: input
                    .get("confidence")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(50)
                    .min(100) as u8,
                task_id: input
                    .get("task_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            };
            if let Some(stream) = &output_stream {
                stream
                    .trace_sender
                    .send(ToolTraceEvent::Ui(
                        riga_kernel::events::RigaEvent::EvidenceAdded {
                            evidence: Box::new(evidence.clone()),
                        },
                    ))
                    .await
                    .map_err(|_| "tool lifecycle stream closed")?;
            }
            serde_json::to_string(&evidence).map_err(|e| e.to_string())
        }
        "link_evidence" => {
            let claim_id = input
                .get("claim_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("link_evidence requires claim_id")?;
            let evidence_id = input
                .get("evidence_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("link_evidence requires evidence_id")?;
            if let Some(stream) = &output_stream {
                stream
                    .trace_sender
                    .send(ToolTraceEvent::Ui(
                        riga_kernel::events::RigaEvent::EvidenceLinked {
                            claim_id: claim_id.into(),
                            evidence_id: evidence_id.into(),
                        },
                    ))
                    .await
                    .map_err(|_| "tool lifecycle stream closed")?;
            }
            Ok(format!("linked evidence {evidence_id} to claim {claim_id}"))
        }
        "remember" => {
            let knowledge = riga_kernel::events::KnowledgeNode {
                id: input
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("knowledge-1")
                    .into(),
                fact: input
                    .get("fact")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("remember requires fact")?
                    .into(),
                source_run_id: String::new(),
                confidence: input
                    .get("confidence")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(50)
                    .min(100) as u8,
            };
            if let Some(stream) = &output_stream {
                stream
                    .trace_sender
                    .send(ToolTraceEvent::Ui(
                        riga_kernel::events::RigaEvent::KnowledgeCreated {
                            knowledge: Box::new(knowledge.clone()),
                        },
                    ))
                    .await
                    .map_err(|_| "tool lifecycle stream closed")?;
            }
            serde_json::to_string(&knowledge).map_err(|e| e.to_string())
        }
        "link_knowledge" => {
            let knowledge_id = input
                .get("knowledge_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("link_knowledge requires knowledge_id")?;
            let evidence_id = input
                .get("evidence_id")
                .and_then(serde_json::Value::as_str)
                .ok_or("link_knowledge requires evidence_id")?;
            if let Some(stream) = &output_stream {
                stream
                    .trace_sender
                    .send(ToolTraceEvent::Ui(
                        riga_kernel::events::RigaEvent::KnowledgeLinked {
                            knowledge_id: knowledge_id.into(),
                            evidence_id: evidence_id.into(),
                        },
                    ))
                    .await
                    .map_err(|_| "tool lifecycle stream closed")?;
            }
            Ok(format!(
                "linked knowledge {knowledge_id} to evidence {evidence_id}"
            ))
        }
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
        "update_graph" => {
            let graph: riga_kernel::task::Graph = serde_json::from_value(input)
                .map_err(|error| format!("update_graph arguments are invalid: {error}"))?;
            graph
                .validate()
                .map_err(|error| format!("update_graph rejected graph: {error}"))?;
            for node in &graph.nodes {
                if crate::catalog::find_agent_profile(&node.profile).is_none() {
                    return Err(format!(
                        "update_graph rejected unknown profile `{}`",
                        node.profile
                    ));
                }
            }
            let (ready, blocked) = {
                let mut runtime = output_stream
                    .as_ref()
                    .ok_or("update_graph requires a live run")?
                    .graph
                    .lock()
                    .await;
                runtime.graph = Some(graph.clone());
                runtime.states = graph
                    .nodes
                    .iter()
                    .map(|node| (node.id.clone(), riga_kernel::task::TaskState::Pending))
                    .collect();
                let ready = graph.ready_nodes(&runtime.states);
                let blocked = graph
                    .nodes
                    .iter()
                    .filter(|node| !ready.contains(&node.id))
                    .map(|node| (node.id.clone(), node.depends_on.clone()))
                    .collect::<Vec<_>>();
                (ready, blocked)
            };
            let ready_count = ready.len();
            if let Some(stream) = &output_stream {
                stream
                    .trace_sender
                    .send(ToolTraceEvent::Ui(
                        riga_kernel::events::RigaEvent::GraphUpdated {
                            graph: Box::new(graph.clone()),
                        },
                    ))
                    .await
                    .map_err(|_| "tool lifecycle stream closed")?;
                for node in &graph.nodes {
                    for dependency in &node.depends_on {
                        stream
                            .trace_sender
                            .send(ToolTraceEvent::Ui(
                                riga_kernel::events::RigaEvent::TaskDependencyAdded {
                                    task_id: node.id.clone(),
                                    depends_on: dependency.clone(),
                                },
                            ))
                            .await
                            .map_err(|_| "tool lifecycle stream closed")?;
                    }
                }
                for task_id in ready {
                    stream
                        .trace_sender
                        .send(ToolTraceEvent::Ui(
                            riga_kernel::events::RigaEvent::TaskRunnable { task_id },
                        ))
                        .await
                        .map_err(|_| "tool lifecycle stream closed")?;
                }
                for (task_id, blocked_by) in blocked {
                    if !blocked_by.is_empty() {
                        stream
                            .trace_sender
                            .send(ToolTraceEvent::Ui(
                                riga_kernel::events::RigaEvent::TaskBlocked {
                                    task_id,
                                    blocked_by,
                                },
                            ))
                            .await
                            .map_err(|_| "tool lifecycle stream closed")?;
                    }
                }
            }
            Ok(format!(
                "Graph updated: {} node(s), {} ready.",
                graph.nodes.len(),
                ready_count
            ))
        }
        "set_model_budget" => {
            let stream = output_stream
                .as_ref()
                .ok_or("set_model_budget requires a live run")?;
            let mut budget = stream.budget.lock().await;
            let summary = budget.apply(&input)?;
            Ok(format!("Model budget updated: {summary}"))
        }
        "grant_tools" => {
            let stream = output_stream
                .as_ref()
                .ok_or("grant_tools requires a live run")?;
            let profile_name = input
                .get("profile")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or("grant_tools requires a profile")?;
            let profile = crate::catalog::find_agent_profile(profile_name)
                .ok_or_else(|| format!("unknown agent `{profile_name}`"))?;
            let tools: Vec<String> = input
                .get("tools")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if tools.is_empty() {
                return Err("grant_tools requires at least one tool name".into());
            }
            let mut grants = stream.tool_grants.lock().await;
            let granted = grants.entry(profile.name.clone()).or_default();
            for tool in &tools {
                if !granted.contains(tool) {
                    granted.push(tool.clone());
                }
            }
            Ok(format!(
                "Granted to `{}` for this run: {}. Effective tools: {}",
                profile.name,
                tools.join(", "),
                allowed_tools_for(&profile, Some(granted.as_slice())).join(", ")
            ))
        }
        "reset_tools" => {
            let stream = output_stream
                .as_ref()
                .ok_or("reset_tools requires a live run")?;
            let mut grants = stream.tool_grants.lock().await;
            match input
                .get("profile")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(profile) => {
                    grants.remove(profile);
                    Ok(format!("Cleared tool grants for `{profile}`"))
                }
                None => {
                    grants.clear();
                    Ok("Cleared all tool grants for this run".into())
                }
            }
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

/// The path a file tool was asked to act on.
///
/// Small local models routinely emit `file` or `filename` instead of `path`, and
/// a strict read made the write fail silently in the transcript that prompted
/// this. The canonical key is preferred; the rest are accepted.
fn path_arg(input: &serde_json::Value) -> Option<&str> {
    ["path", "file", "filename", "file_path", "filepath"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(serde_json::Value::as_str))
}

/// A tool-argument error that tells the model what to send instead.
///
/// Small models ignore the schema and invent shapes such as `{"files": [...]}`
/// or `{"commands": [...]}`. A terse `write requires path` leaves them guessing
/// again (and they often just retry the same call). Naming the expected keys and
/// the ones actually sent gives them something concrete to correct against.
fn argument_error(tool: &str, expected: &str, input: &serde_json::Value) -> String {
    let keys: Vec<&str> = input
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    format!(
        "{tool} needs {expected}; received keys: [{}]",
        keys.join(", ")
    )
}

/// The directory a `cd <dir> && …` command switches into, when it starts with one.
fn cd_target(command: &str) -> Option<String> {
    let rest = command.trim().strip_prefix("cd ")?;
    let target = rest
        .split("&&")
        .next()?
        .trim()
        .trim_matches(|character| character == '"' || character == '\'');
    (!target.is_empty()).then(|| target.to_owned())
}

/// Refuse an install command that has no manifest to install from.
///
/// A model that runs `npm install` before writing `package.json` gets a shell
/// error that reads like a harness failure, and it wastes an approval prompt.
/// Naming what is missing teaches it the ordering instead.
fn install_preflight(root: &std::path::Path, command: &str) -> Option<String> {
    let lowered = command.to_ascii_lowercase();
    let node_install = [
        "npm install",
        "npm ci",
        "npm i ",
        "yarn install",
        "yarn add",
        "pnpm install",
        "pnpm add",
        "bun install",
    ];
    let manifests: &[&str] = if lowered.contains("pip install") || lowered.contains("pip3 install")
    {
        &["requirements.txt", "pyproject.toml", "setup.py"]
    } else if node_install.iter().any(|needle| lowered.contains(needle)) {
        &["package.json"]
    } else {
        return None;
    };
    // A manifest in the workspace root or in the `cd` target satisfies the check.
    let target = cd_target(command)
        .map(|dir| root.join(dir))
        .unwrap_or_else(|| root.to_path_buf());
    if manifests
        .iter()
        .any(|manifest| target.join(manifest).is_file() || root.join(manifest).is_file())
    {
        return None;
    }
    Some(format!(
        "no {} exists yet; create it with `write` before running an install command",
        manifests[0]
    ))
}

/// The body a `write` was asked to store.
///
/// Phi-4 emits `text` where the schema says `content`; without this the call
/// fails with "write requires content" and the model retries until the run
/// exhausts its turns. The canonical key is preferred; the rest are accepted.
fn content_arg(input: &serde_json::Value) -> Option<&str> {
    ["content", "text", "body", "contents", "data"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(serde_json::Value::as_str))
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

/// The server-side sink for one run.
///
/// Every event is appended to the run's durable journal and then broadcast, so
/// a run keeps going whether or not a client is watching, and a client that
/// reconnects can replay what it missed and then follow the run live. The
/// journal is written first so a frame that reaches a subscriber is already
/// durable.
pub(crate) struct RunChannel {
    journal: Option<riga_kernel::persistence::EventJournal>,
    events: tokio::sync::broadcast::Sender<RigaEventEnvelope>,
    run_id: String,
    session_id: String,
    sequence: u64,
}

impl RunChannel {
    fn new(
        run_id: &str,
        session_id: &str,
        events: tokio::sync::broadcast::Sender<RigaEventEnvelope>,
    ) -> Self {
        Self {
            journal: riga_kernel::persistence::EventJournal::open(run_journal_path(run_id)).ok(),
            events,
            run_id: run_id.to_owned(),
            session_id: session_id.to_owned(),
            sequence: 1,
        }
    }

    /// A channel that only broadcasts, for tests that must not touch the data
    /// directory. Production always journals.
    #[cfg(test)]
    pub(crate) fn without_journal(
        run_id: &str,
        session_id: &str,
        events: tokio::sync::broadcast::Sender<RigaEventEnvelope>,
    ) -> Self {
        Self {
            journal: None,
            events,
            run_id: run_id.to_owned(),
            session_id: session_id.to_owned(),
            sequence: 1,
        }
    }

    /// Broadcast one event, journaling it first unless it is ephemeral.
    /// `RunStarted` owns sequence 1.
    pub(crate) fn emit(&mut self, event: RigaEvent) {
        let envelope = envelope(&self.run_id, &self.session_id, self.sequence, event);
        self.sequence += 1;
        // Token and tool-output deltas are high-frequency and reconstructible:
        // the final reply and each tool result are durable, so journaling every
        // delta would only rewrite the run file once per token. They are still
        // broadcast, so a watching client sees them live.
        if !is_ephemeral(&envelope.event)
            && let Some(journal) = self.journal.as_mut()
            && let Err(error) = journal.append(envelope.clone())
        {
            tracing::warn!(?error, "run event could not be journaled");
        }
        // No receiver just means nobody is watching right now; the journal holds
        // the event for whichever client connects next.
        let _ = self.events.send(envelope);
    }

    fn emit_trace(&mut self, trace: ToolTraceEvent) {
        let event = match trace {
            ToolTraceEvent::Started(call) => RigaEvent::ToolCallStarted { call },
            ToolTraceEvent::Output { call_id, delta } => {
                RigaEvent::ToolOutputDelta { call_id, delta }
            }
            ToolTraceEvent::Completed(trace) => RigaEvent::ToolResult {
                result: serde_json::json!({
                    "call_id": trace
                        .call
                        .get("call_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("tool-call"),
                    "name": trace.name,
                    "output": trace.output,
                    "ok": trace.ok,
                    "task_id": trace.call.get("task_id").cloned(),
                }),
            },
            ToolTraceEvent::Ui(event) => event,
        };
        self.emit(event);
    }
}

/// Run one agent turn to completion, writing every event to `channel`.
///
/// This is the detached half of a run: it never touches a socket, so a client
/// disconnecting does not interrupt it. Approvals arrive through the shared
/// broker and cancellation through `evidence`, both independent of any socket.
#[allow(clippy::too_many_arguments)]
async fn run_provider(
    channel: &mut RunChannel,
    broker: &ApprovalBroker,
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    local_models: &std::sync::Arc<crate::local_model::LocalModelRuntime>,
    history: &[ConversationTurn],
    evidence: &RunEvidence,
) -> Option<String> {
    channel.emit(RigaEvent::RunStarted);
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
        let local_models = config.is_local().then(|| local_models.clone());
        let evidence = (*evidence).clone();
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
                local_models.as_ref(),
                &evidence,
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
            evidence,
        ))
    };
    // Nothing is read from a socket here: approvals arrive through the shared
    // broker and cancellation through `evidence`. A dropped client therefore
    // cannot stop this loop.
    let result = loop {
        tokio::select! {
            trace = trace_receiver.recv() => {
                if let Some(trace) = trace {
                    channel.emit_trace(trace);
                }
            }
            result = &mut provider_call => break result,
        }
    };
    while let Ok(trace) = trace_receiver.try_recv() {
        channel.emit_trace(trace);
    }
    match result {
        Ok(result) => {
            // The guard: a final answer must not claim files were written or
            // commands run when no mutating tool succeeded. The loops already
            // try to nudge the model into doing the work; this catches a claim
            // that survives that, such as from the Responses path, which runs
            // no tools at all.
            let output = if claims_work(&result.output) && !evidence.has_work() {
                tracing::warn!(
                    run_id = %channel.run_id,
                    "final answer claimed work but no mutating tool ran; annotating"
                );
                format!(
                    "{}\n\n[verification] No successful file write or command was recorded in this run, so the changes described above were not actually made.",
                    result.output
                )
            } else {
                result.output
            };
            // When the model streamed token by token, the reply is already in the
            // client; resending it as one delta would duplicate it. A provider
            // that answers in one piece still needs this delta.
            if !evidence.was_streamed() {
                channel.emit(RigaEvent::TextDelta {
                    delta: output.clone(),
                });
            }
            channel.emit(RigaEvent::RunCompleted {
                output: output.clone(),
            });
            Some(output)
        }
        Err(error) => {
            channel.emit(RigaEvent::RunFailed { message: error });
            None
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
    evidence: &RunEvidence,
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
            evidence,
        )
        .await;
    }
    match config.api {
        ProviderApi::Responses => {
            call_responses_api(
                config,
                workspace_root,
                &coding_agent_system_prompt(workspace_root),
                None,
                prompt,
                mcp_runtime,
                trace_sender,
                history,
                evidence,
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
                evidence,
            )
            .await
        }
    }
}
#[allow(clippy::too_many_arguments)]
async fn call_chat_with_tools(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
    history: &[ConversationTurn],
    broker: &ApprovalBroker,
    evidence: &RunEvidence,
) -> Result<AgentResult, String> {
    run_chat_loop(
        config,
        workspace_root,
        &coding_agent_system_prompt(workspace_root),
        None,
        prompt,
        mcp_runtime,
        trace_sender,
        history,
        0,
        broker,
        None,
        evidence,
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
    task_id: Option<&str>,
    evidence: &RunEvidence,
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
    let mut nudges = 0usize;
    // Reset by any successful tool call, so only a genuinely stuck loop stops.
    let mut consecutive_failures = 0usize;
    for _ in 0..24 {
        // Honour a cancellation between turns; an in-flight request cannot be
        // interrupted, but the next one is not started.
        if evidence.is_cancelled() {
            return Err("run cancelled".into());
        }
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
        if !status.is_success() {
            let body = response.text().await.map_err(|e| e.to_string())?;
            return Err(format!(
                "provider returned HTTP {status}: {}",
                redact_body(&body)
            ));
        }
        // A streaming provider (the normal OpenAI-compatible shape) answers with
        // server-sent events; forward the text as it arrives so a remote model
        // streams like a local one. A gateway that ignores `stream` and returns
        // one JSON body is parsed the old way.
        let streaming = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
        let message = if streaming {
            read_streamed_message(response, &trace_sender, evidence).await?
        } else {
            let body = response.text().await.map_err(|e| e.to_string())?;
            let value: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| format!("invalid provider response: {e}"))?;
            value
                .get("choices")
                .and_then(serde_json::Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("message"))
                .cloned()
                .ok_or_else(|| "provider returned no choices".to_owned())?
        };
        let tool_calls = message
            .get("tool_calls")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        if tool_calls.is_empty() {
            let text = message
                .get("content")
                .and_then(content_text)
                .ok_or_else(|| "provider returned no assistant text".to_owned())?;
            // Nudge a claim the run cannot back before accepting the answer.
            if claims_work(&text) && !evidence.has_work() && nudges < MAX_CLAIM_NUDGES {
                nudges += 1;
                messages.push(serde_json::json!({ "role": "assistant", "content": text }));
                messages.push(serde_json::json!({ "role": "user", "content": CLAIM_NUDGE }));
                continue;
            }
            return Ok(AgentResult { output: text });
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
                    // Which subagent this call belongs to; null for the
                    // orchestrator. The UI nests task-scoped tools.
                    "task_id": task_id,
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
                            None,
                            evidence,
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
                            None,
                            evidence,
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
                                graph: evidence.graph(),
                                budget: evidence.budget(),
                                tool_grants: evidence.tool_grants(),
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
            if ok {
                consecutive_failures = 0;
            } else {
                consecutive_failures += 1;
            }
            evidence.record(&call.name, ok);
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
                "content": crate::catalog::truncate_tool_result(output.clone(), crate::catalog::MAX_TOOL_RESULT_CHARS),
            }));
            // Stop a model that keeps re-sending a call that cannot succeed.
            if consecutive_failures >= MAX_CONSECUTIVE_TOOL_FAILURES {
                return Err(format!(
                    "the model called `{}` {consecutive_failures} times in a row and each attempt failed; stopping so it does not loop. Last error: {output}",
                    call.name
                ));
            }
        }
    }
    Err("provider exceeded the maximum tool-call turns".into())
}

/// True when a `task` call is asking to run a subagent rather than to list or
/// create a durable task record.
fn is_subagent_dispatch(input: &serde_json::Value) -> bool {
    matches!(
        input.get("action").and_then(serde_json::Value::as_str),
        Some("dispatch") | Some("agent") | Some("run")
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
    // A mention can appear anywhere, not only at the very start: users write
    // "Build X ... @build make sure ...". Find the first `@name` that names a
    // known profile and hand it the whole request (with the mention removed) as
    // its task. An `@` that names no profile (an email address, say) is skipped.
    let mut search = prompt;
    while let Some(at) = search.find('@') {
        let after = &search[at + 1..];
        let name: String = after
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
            })
            .collect();
        if !name.is_empty()
            && let Some(profile) = crate::catalog::find_agent_profile(&name.to_lowercase())
        {
            let before = &search[..at];
            let rest = &after[name.len()..];
            let task = format!("{} {}", before.trim(), rest.trim())
                .trim()
                .to_owned();
            if task.is_empty() {
                return None;
            }
            return Some((profile.name, task));
        }
        search = &search[at + 1..];
    }
    None
}

/// Process-wide counter for subagent task ids.
static TASK_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Run a subagent to completion and return its result as the tool result.
///
/// The child gets its own context (empty history), the profile's system prompt,
/// and a restricted tool set, so its file reads never enter the orchestrator's
/// window — only the summary does. `TaskStarted`/`TaskCompleted` frames let the
/// UI render the delegation.
#[allow(clippy::too_many_arguments)]
async fn dispatch_subagent(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    input: &serde_json::Value,
    mcp_runtime: &McpRuntime,
    trace_sender: &mpsc::Sender<ToolTraceEvent>,
    depth: usize,
    broker: &ApprovalBroker,
    local_models: Option<&std::sync::Arc<crate::local_model::LocalModelRuntime>>,
    evidence: &RunEvidence,
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
    let graph_node_id = input
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(node_id) = &graph_node_id {
        let graph = evidence.graph();
        let mut runtime = graph.lock().await;
        let published = runtime
            .graph
            .as_ref()
            .ok_or("task dispatch references a node but no graph has been published")?;
        let node = published
            .nodes
            .iter()
            .find(|node| &node.id == node_id)
            .ok_or_else(|| format!("task dispatch references unknown graph node `{node_id}`"))?;
        if node.profile != profile.name {
            return Err(format!(
                "graph node `{node_id}` requires profile `{}`, not `{}`",
                node.profile, profile.name
            ));
        }
        let blocked_by: Vec<String> = node
            .depends_on
            .iter()
            .filter(|dependency| {
                runtime.states.get(*dependency) != Some(&riga_kernel::task::TaskState::Completed)
            })
            .cloned()
            .collect();
        if !blocked_by.is_empty() {
            let _ = trace_sender
                .send(ToolTraceEvent::Ui(
                    riga_kernel::events::RigaEvent::TaskBlocked {
                        task_id: node_id.clone(),
                        blocked_by: blocked_by.clone(),
                    },
                ))
                .await;
            return Err(format!(
                "task `{node_id}` is blocked by incomplete dependencies: {}",
                blocked_by.join(", ")
            ));
        }
        runtime
            .states
            .insert(node_id.clone(), riga_kernel::task::TaskState::Running);
    }
    let task_id = graph_node_id.clone().unwrap_or_else(|| {
        format!(
            "task-{}",
            TASK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    });
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
        parent_id: input
            .get("parent_task_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
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

    let _ = trace_sender
        .send(ToolTraceEvent::Ui(
            riga_kernel::events::RigaEvent::TaskStatus {
                task_id: task_id.clone(),
                state: riga_kernel::task::TaskState::Running,
                elapsed_ms: 0,
            },
        ))
        .await;

    let allowed = {
        let grants = evidence.tool_grants();
        let guard = grants.lock().await;
        allowed_tools_for(&profile, guard.get(&profile.name).map(Vec::as_slice))
    };
    let system_prompt = subagent_system_prompt(&profile, workspace_root);
    // A local run dispatches local children; a remote run dispatches remote
    // children, so a subagent never crosses the provider boundary. The child's
    // loop is tagged with its task id so its tool calls nest in the UI.
    let outcome = match (config.is_local(), local_models) {
        (true, Some(local_models)) => {
            Box::pin(run_local_loop(
                &child_config,
                workspace_root,
                &system_prompt,
                Some(&allowed),
                prompt,
                mcp_runtime,
                trace_sender.clone(),
                local_models,
                &[],
                depth + 1,
                broker,
                Some(&task_id),
                evidence,
            ))
            .await
        }
        _ => {
            if child_config.api == ProviderApi::Responses {
                Box::pin(call_responses_api(
                    &child_config,
                    workspace_root,
                    &system_prompt,
                    Some(&allowed),
                    prompt,
                    mcp_runtime,
                    trace_sender.clone(),
                    &[],
                    evidence,
                ))
                .await
            } else {
                Box::pin(run_chat_loop(
                    &child_config,
                    workspace_root,
                    &system_prompt,
                    Some(&allowed),
                    prompt,
                    mcp_runtime,
                    trace_sender.clone(),
                    &[],
                    depth + 1,
                    broker,
                    Some(&task_id),
                    evidence,
                ))
                .await
            }
        }
    };

    let (ok, result) = match outcome {
        Ok(result) => (true, result.output),
        Err(error) => (false, error),
    };
    if graph_node_id.is_some() {
        evidence.graph().lock().await.states.insert(
            task_id.clone(),
            if ok {
                riga_kernel::task::TaskState::Completed
            } else {
                riga_kernel::task::TaskState::Failed
            },
        );
    }
    let terminal_state = if ok {
        riga_kernel::task::TaskState::Completed
    } else {
        riga_kernel::task::TaskState::Failed
    };
    let _ = trace_sender
        .send(ToolTraceEvent::Ui(
            riga_kernel::events::RigaEvent::TaskStatus {
                task_id: task_id.clone(),
                state: terminal_state,
                elapsed_ms: 0,
            },
        ))
        .await;
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
fn subagent_system_prompt(
    profile: &crate::catalog::AgentProfile,
    workspace_root: &std::path::Path,
) -> String {
    let mut prompt = format!(
        "You are the RIGA `{}` subagent. {}\n\nRules:\n",
        profile.name, profile.purpose
    );
    for rule in &profile.system_rules {
        prompt.push_str(&format!("- {rule}\n"));
    }
    prompt.push_str(&format!(
        "\nWorkspace root: {}\n\
         You were dispatched by the orchestrator: work autonomously, do not ask the user questions, and return a concise result it can use.\n",
        workspace_root.display()
    ));
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
/// The tools a subagent may call: the run-state tools every profile has, the
/// profile's declared tools, and any tools granted for this run.
///
/// The declared `tools` are the source of truth, so a read-only profile that
/// declares `bash` may run shell commands — each one still goes through the
/// approval gate. Read-only profiles simply do not declare `write` or `task`.
fn allowed_tools_for(
    profile: &crate::catalog::AgentProfile,
    grants: Option<&[String]>,
) -> Vec<String> {
    let mut allowed = vec![
        "update_plan".to_owned(),
        "update_todos".to_owned(),
        "update_graph".to_owned(),
        "set_model_budget".to_owned(),
        "grant_tools".to_owned(),
        "reset_tools".to_owned(),
    ];
    for tool in &profile.tools {
        let base = tool.split('(').next().unwrap_or(tool).trim();
        if !base.is_empty() && !allowed.iter().any(|name| name == base) {
            allowed.push(base.to_owned());
        }
    }
    if let Some(grants) = grants {
        for tool in grants {
            if !allowed.iter().any(|name| name == tool) {
                allowed.push(tool.clone());
            }
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
/// How many times a local turn is asked to re-emit an unparseable tool call.
const MAX_TOOL_CALL_RETRIES: usize = 2;

/// How many tool executions may fail in a row before the local loop gives up.
///
/// A small model that keeps re-sending a call with the wrong arguments otherwise
/// burns every turn, and the run looks stuck at "thinking" for many minutes.
const MAX_CONSECUTIVE_TOOL_FAILURES: usize = 3;

/// Cap on tool calls in one local run, across every turn. A model that keeps
/// globbing for files that do not exist would otherwise spend the whole turn
/// budget; this stops it even when every call is a *different* pattern.
const LOCAL_MAX_TOOL_CALLS: usize = 16;

/// How many times the exact same call (name + arguments) may appear before the
/// run stops. A model that repeats a call it already has the result for is
/// looping, not working.
const MAX_REPEATED_TOOL_CALLS: usize = 2;

const LOCAL_TOOL_CALL_RETRY: &str = "Your reply contained a <tool_call> block that was not valid JSON; it was probably cut off. \
Reply with exactly ONE smaller tool call whose <tool_call> block is valid JSON. Prefer several small `write` calls over one large one.";

/// Wall-clock budget for the whole run. The per-turn output cap, turn wall clock,
/// idle window, turn count, and run token budget now come from the run's
/// `LocalBudget` (set by `set_model_budget`), so they are not constants here.
const LOCAL_RUN_WALL_CLOCK: std::time::Duration = std::time::Duration::from_secs(600);

#[derive(Debug, PartialEq)]
struct ParsedToolCall {
    name: String,
    arguments: serde_json::Value,
}

/// Remove Qwen3-style ` thinking…</think>` reasoning from a reply.
///
/// The reasoning is not part of the answer; leaving it in would show up as prose
/// and be fed back to the model on the next turn. An unterminated block (a turn
/// cut off mid-thought) drops the tail rather than leaking half of it.
fn strip_reasoning(text: &str) -> String {
    // Angle-bracket tags written as escapes: literal tag text is mangled by tooling.
    const OPEN: &str = "\u{3c}think\u{3e}";
    const CLOSE: &str = "\u{3c}/think\u{3e}";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    // A template-opened reasoning block (Qwen3 `add_generation_prompt`) carries
    // no opening tag in the generated text: the reply starts inside the block, so
    // the first marker is the close tag. Drop everything up to and including it.
    if let Some(end) = rest.find(CLOSE)
        && rest.find(OPEN).is_none_or(|open| end < open)
    {
        rest = &rest[end + CLOSE.len()..];
    }
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        match after.find(CLOSE) {
            Some(end) => rest = &after[end + CLOSE.len()..],
            None => return out.trim().to_owned(),
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
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
    let (prose, mut calls) = parse_tagged_tool_calls(text);
    let (prose, fenced) = parse_fenced_tool_calls(&prose);
    calls.extend(fenced);
    let (prose, keyed) = parse_keyed_lines(&prose);
    calls.extend(keyed);
    (prose, calls)
}

/// Extract `<tool_call>{…}</tool_call>` blocks, the format the prompt asks for.
fn parse_tagged_tool_calls(text: &str) -> (String, Vec<ParsedToolCall>) {
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

/// Extract tool calls the model wrote as a fenced JSON block instead of a
/// `<tool_call>` block.
///
/// Hermes 3 and similar models routinely answer with a `json` fenced block
/// (`{"action":"dispatch","agent":"plan",…}`) and never call the tool otherwise.
/// Only JSON fences whose body parses to a recognizable tool call are consumed;
/// anything else stays prose.
fn parse_fenced_tool_calls(text: &str) -> (String, Vec<ParsedToolCall>) {
    let mut calls = Vec::new();
    let mut prose = String::with_capacity(text.len());
    let mut scan = text;
    while let Some(open) = scan.find("```") {
        let after = &scan[open + 3..];
        let Some(close) = after.find("```") else {
            break;
        };
        let inner = &after[..close];
        let (tag, body) = match inner.find('\n') {
            Some(newline) => (inner[..newline].trim(), &inner[newline + 1..]),
            None => ("", inner),
        };
        // A single-line fence may still carry a `json` tag before the object.
        let body = body.trim_start();
        let body = if tag.is_empty() {
            body.strip_prefix("json")
                .map(str::trim_start)
                .unwrap_or(body)
        } else {
            body
        };
        // A `json` fence holds a JSON object; a `text` fence (Hermes) holds the
        // loose `tool key:value …` form. Try both; anything else stays prose.
        let call = parse_loose_tool_call(body.trim());
        match call {
            Some(call) => {
                prose.push_str(&scan[..open]);
                calls.push(call);
            }
            None => prose.push_str(&scan[..open + 3 + close + 3]),
        }
        scan = &after[close + 3..];
    }
    prose.push_str(scan);
    (prose.trim().to_owned(), calls)
}

/// Try the JSON shape first, then the loose `tool key:value …` shape.
fn parse_loose_tool_call(body: &str) -> Option<ParsedToolCall> {
    parse_one_local_tool_call(body).or_else(|| parse_key_value_tool_call(body))
}

/// Argument keys the loose `tool key:value key:value` form may carry.
///
/// A token is a key only when it is one of these, so a colon inside a value (a
/// URL, a `file:` path) is never mistaken for a new key.
const LOOSE_ARGUMENT_KEYS: &[&str] = &[
    "action",
    "agent",
    "prompt",
    "description",
    "path",
    "content",
    "command",
    "pattern",
    "query",
    "url",
    "name",
    "title",
    "status",
    "task_id",
    "node_id",
    "parent_task_id",
];

/// Parse the loose form Hermes emits when it ignores the JSON protocol:
/// `task action:dispatch agent:plan prompt:Plan the steps …`.
///
/// The first token is the tool name and the rest is `key:value` pairs where a
/// value runs until the next recognized key. The first token after the name must
/// be a key, so ordinary prose (`read the file`) is not mistaken for a call.
fn parse_key_value_tool_call(line: &str) -> Option<ParsedToolCall> {
    let line = line.trim();
    let (name, rest) = line.split_once(char::is_whitespace)?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let mut args = serde_json::Map::new();
    let mut key: Option<&str> = None;
    let mut value = String::new();
    let mut first = true;
    for token in rest.split_whitespace() {
        match split_loose_key(token) {
            Some((next_key, next_value)) => {
                if let Some(previous) = key.take() {
                    args.insert(
                        previous.to_owned(),
                        serde_json::Value::String(value.trim().to_owned()),
                    );
                }
                key = Some(next_key);
                value = next_value.to_owned();
            }
            None => {
                if first {
                    return None;
                }
                value.push(' ');
                value.push_str(token);
            }
        }
        first = false;
    }
    let last = key?;
    args.insert(
        last.to_owned(),
        serde_json::Value::String(value.trim().to_owned()),
    );

    let mut name = name.to_owned();
    if matches!(name.as_str(), "dispatch" | "agent" | "run") {
        name = "task".into();
    }
    if name == "task" {
        args.entry("action")
            .or_insert_with(|| serde_json::json!("dispatch"));
    }
    Some(ParsedToolCall {
        name,
        arguments: serde_json::Value::Object(args),
    })
}

/// `key:value` when `key` is a recognized argument key.
fn split_loose_key(token: &str) -> Option<(&str, &str)> {
    let (key, value) = token.split_once(':')?;
    LOOSE_ARGUMENT_KEYS.contains(&key).then_some((key, value))
}

/// Consume any whole line that is a loose `tool key:value …` call.
fn parse_keyed_lines(text: &str) -> (String, Vec<ParsedToolCall>) {
    let mut calls = Vec::new();
    let mut kept = Vec::new();
    for line in text.lines() {
        match parse_key_value_tool_call(line) {
            Some(call) => calls.push(call),
            None => kept.push(line),
        }
    }
    (kept.join("\n").trim().to_owned(), calls)
}

/// Escape literal control characters inside JSON string values.
///
/// Small models routinely paste a multi-line file body straight into the JSON of
/// a tool call; a raw newline or tab inside a string is invalid JSON, so the
/// whole call would be dropped. Only characters inside strings are touched, and
/// existing escapes are preserved.
fn escape_json_control_chars(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 16);
    let mut in_string = false;
    let mut escaped = false;
    for ch in raw.chars() {
        if in_string && escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => {
                escaped = true;
                out.push(ch);
            }
            '"' => {
                in_string = !in_string;
                out.push(ch);
            }
            '\n' if in_string => out.push_str("\\n"),
            '\r' if in_string => out.push_str("\\r"),
            '\t' if in_string => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/// Parse a tool-call object leniently.
///
/// Models emit malformed JSON in predictable ways: real newlines inside strings,
/// prose around the object, or a trailing comma. Try the plain parse, the
/// control-char-escaped parse, then the outermost `{…}` substring.
fn parse_json_lenient(raw: &str) -> Option<serde_json::Value> {
    let raw = raw.trim();
    if let Ok(value) = serde_json::from_str(raw) {
        return Some(value);
    }
    let escaped = escape_json_control_chars(raw);
    if let Ok(value) = serde_json::from_str(&escaped) {
        return Some(value);
    }
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    let slice = &raw[start..=end];
    if let Ok(value) = serde_json::from_str(slice) {
        return Some(value);
    }
    serde_json::from_str(&escape_json_control_chars(slice)).ok()
}

fn parse_one_local_tool_call(raw: &str) -> Option<ParsedToolCall> {
    // A model that writes a file body with real newlines produces invalid JSON
    // (control characters are not allowed inside a JSON string). Retry once with
    // those characters escaped rather than failing the whole run.
    let candidate = parse_json_lenient(raw)?;
    let object = candidate.as_object()?;

    // An explicit `name` is authoritative.
    if let Some(name) = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let mut name = name.to_owned();
        let mut arguments = object
            .get("arguments")
            .or_else(|| object.get("parameters"))
            .or_else(|| object.get("input"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        // Small models often emit the arguments as a JSON *string* containing the
        // object. Unwrap that instead of failing the run over a quoting slip.
        arguments = match arguments {
            serde_json::Value::String(ref inner) => {
                serde_json::from_str(inner).unwrap_or(arguments)
            }
            other => other,
        };
        // Small local models sometimes mistake the task action for the tool name
        // and emit {"name":"dispatch","agent":...}. Normalize that common
        // shape before tool lookup so it becomes the registered `task` tool.
        if matches!(name.as_str(), "dispatch" | "agent" | "run") {
            name = "task".into();
            if let Some(object) = arguments.as_object_mut() {
                object
                    .entry("action")
                    .or_insert_with(|| serde_json::json!("dispatch"));
            }
        }
        return Some(ParsedToolCall { name, arguments });
    }

    // No `name`: infer the tool from the argument shape. Models that ignore the
    // `<tool_call>{"name": …}` wrapper still tend to use the right argument keys.
    infer_tool_call(object)
}

/// Infer a tool call from an object that carries only the arguments.
///
/// Models that drop the `{"name": …}` wrapper still use the right argument keys,
/// so match the object against the built-in schemas: a tool is a candidate when
/// every key is one of its properties and its `required` set is satisfied; the
/// highest-scoring candidate wins. A JSON example in prose matches no schema and
/// is left alone.
fn infer_tool_call(object: &serde_json::Map<String, serde_json::Value>) -> Option<ParsedToolCall> {
    if object.is_empty() {
        return None;
    }
    let mut best: Option<(String, usize)> = None;
    for definition in builtin_tool_definitions() {
        let Some(properties) = definition
            .parameters
            .get("properties")
            .and_then(serde_json::Value::as_object)
        else {
            continue;
        };
        // Every key must be a property of this tool, or the shape is not it.
        if object.keys().any(|key| !properties.contains_key(key)) {
            continue;
        }
        let required: Vec<&str> = definition
            .parameters
            .get("required")
            .and_then(serde_json::Value::as_array)
            .map(|items| items.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default();
        if !required.iter().all(|key| object.contains_key(*key)) {
            continue;
        }
        // Prefer the tool that explains the most keys and requires the most.
        let score = object.len() + required.len();
        if best
            .as_ref()
            .is_none_or(|(_, best_score)| score > *best_score)
        {
            best = Some((definition.name.clone(), score));
        }
    }
    let (name, _) = best?;
    let mut arguments = serde_json::Value::Object(object.clone());
    if name == "task"
        && let Some(map) = arguments.as_object_mut()
    {
        map.entry("action")
            .or_insert_with(|| serde_json::json!("dispatch"));
    }
    Some(ParsedToolCall { name, arguments })
}

/// Tool instructions for a local model, appended to the coding-agent prompt.
///
/// The curated GGUFs are general-purpose instruction models without a native
/// tool-call template, so the protocol is stated explicitly. `<tool_call>` is the
/// Hermes/Qwen convention these checkpoints were tuned on, which is why it is
/// used here instead of inventing a new one.
fn local_tool_instructions(definitions: &[rig_core::completion::ToolDefinition]) -> String {
    // The exact shape Qwen2.5/3 and Hermes 3 templates emit when they are given
    // tools: a `# Tools` section, the signatures inside `<tools>`, then the
    // `<tool_call>` instruction. Matching it is what makes the model actually
    // emit a call instead of inventing a shape.
    let mut text = String::from(
        "\n\n# Tools\n\nYou may call one or more functions to assist with the user query.\n\n\
         You are provided with function signatures within <tools></tools> XML tags:\n<tools>",
    );
    for definition in definitions {
        let entry = serde_json::json!({
            "type": "function",
            "function": {
                "name": definition.name,
                "description": definition.description,
                "parameters": definition.parameters,
            }
        });
        text.push('\n');
        text.push_str(&serde_json::to_string(&entry).unwrap_or_default());
    }
    text.push_str(
        "\n</tools>\n\n\
         For each function call, return a json object with function name and arguments within \
         <tool_call></tool_call> XML tags, for example:\n\
         <tool_call>\n{\"name\": \"bash\", \"arguments\": {\"command\": \"ls -la\"}}\n</tool_call>\n\n\
         Use the exact function and argument names from `parameters`. Call one function per reply \
         and wait for its result. Emit the call directly, not inside a markdown code fence. \
         `task` is the tool name; `dispatch` is only its `action` value — never emit a tool named \
         `dispatch`.\n\n\
         Examples:\n\
         <tool_call>\n{\"name\": \"read\", \"arguments\": {\"path\": \"src/app.js\"}}\n</tool_call>\n\
         <tool_call>\n{\"name\": \"bash\", \"arguments\": {\"command\": \"npm install\"}}\n</tool_call>\n\
         <tool_call>\n{\"name\": \"task\", \"arguments\": {\"action\": \"dispatch\", \"agent\": \"plan\", \"prompt\": \"plan it\"}}\n</tool_call>\n\n\
         To pick a capability tier or resize this run, call `set_model_budget` \
         ({\"tier\": \"compact\"} or {\"enlarge\": true}); it takes effect next turn. To let a \
         subagent use a tool it lacks, call `grant_tools` ({\"profile\": \"plan\", \"tools\": \
         [\"bash\"]}); the user is asked to approve. `reset_tools` clears those grants.\n\n\
         If a function returns no matches, an empty result, or an error, do not repeat the same \
         call. Try one genuinely different call, then answer with what you have. When you have \
         the final answer, reply with plain prose and no tool_call block.\n",
    );
    text
}

#[allow(clippy::too_many_arguments)]
async fn call_local_model(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
    local_models: &std::sync::Arc<crate::local_model::LocalModelRuntime>,
    history: &[ConversationTurn],
    broker: &ApprovalBroker,
    evidence: &RunEvidence,
) -> Result<AgentResult, String> {
    run_local_loop(
        config,
        workspace_root,
        &coding_agent_system_prompt(workspace_root),
        None,
        prompt,
        mcp_runtime,
        trace_sender,
        local_models,
        history,
        0,
        broker,
        None,
        evidence,
    )
    .await
}

/// Tools that only record run state for the UI. A turn whose only calls are
/// these has nothing left to do, so the loop finishes with the turn's prose.
fn is_record_only_tool(name: &str) -> bool {
    matches!(name, "update_plan" | "update_todos" | "update_graph")
}

/// The local-model tool loop, shaped like `run_chat_loop` so the orchestrator
/// and a dispatched subagent share it with different prompts and tool sets.
#[allow(clippy::too_many_arguments)]
async fn run_local_loop(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    system_prompt: &str,
    allowed_tools: Option<&[String]>,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
    local_models: &std::sync::Arc<crate::local_model::LocalModelRuntime>,
    history: &[ConversationTurn],
    depth: usize,
    broker: &ApprovalBroker,
    task_id: Option<&str>,
    evidence: &RunEvidence,
) -> Result<AgentResult, String> {
    // Built-ins first so the prompt's `<tools>` block lists them, then MCP tools.
    let mut definitions = builtin_tool_definitions();
    definitions.extend(mcp_runtime.tool_definitions().await);
    if let Some(allowed) = allowed_tools {
        definitions.retain(|definition| allowed.iter().any(|name| name == &definition.name));
    }
    let system = format!("{}{}", system_prompt, local_tool_instructions(&definitions));
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
    let mut nudges = 0usize;
    // Bounded, so a model stuck on an unparseable tool call cannot spend the
    // whole turn budget regenerating.
    let mut tool_call_retries = 0usize;
    // Reset by any successful tool call, so only a genuinely stuck loop stops.
    let mut consecutive_failures = 0usize;
    // Whole-run budgets, so a small model cannot generate for tens of minutes
    // while the UI sits at "thinking".
    let run_deadline = std::time::Instant::now() + LOCAL_RUN_WALL_CLOCK;
    let mut generated_total = 0usize;
    // Across turns: a call the model already made must not be re-run.
    let mut tool_calls_total = 0usize;
    let mut seen_calls: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut turn = 0usize;
    loop {
        // Re-read the budget each turn so a `set_model_budget` call mid-run takes
        // effect on the next turn.
        let budget = *evidence.budget().lock().await;
        if turn >= budget.max_turns {
            return Err(format!(
                "the local model reached the {}-turn budget without finishing; ask a narrower question or raise the budget with set_model_budget.",
                budget.max_turns
            ));
        }
        turn += 1;
        // Generation is blocking C, so it runs on the blocking pool. The future
        // is awaited directly: nothing else in this task needs the executor, and
        // a dropped run would otherwise leave a context mid-decode.
        let request = messages.clone();
        let runtime = local_models.clone();
        // Run-scoped stop flag. `CancelRun` cannot reach it yet (see the note at
        // call site), so it is currently always false; passing it explicitly
        // keeps the generate contract intact for when cancellation is wired.
        // A cancel that arrives while the model is mid-decode stops it at the
        // next token instead of after the whole reply.
        if evidence.is_cancelled() {
            return Err("run cancelled".into());
        }
        let cancel = evidence.cancel_flag();
        // Stream this turn's tokens as they decode, keeping the tool-call block
        // out of the visible reply.
        let stream_sender = trace_sender.clone();
        let streamed_flag = evidence.streamed();
        let worker_cancel = cancel.clone();
        let turn_deadline = std::cmp::min(
            run_deadline,
            std::time::Instant::now() + std::time::Duration::from_secs(budget.turn_seconds),
        );
        // Set by `generate` once the prompt is formatted: whether the template
        // opened a reasoning block. The streamer reads it on its first token.
        let reasoning_expected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reasoning_flag = reasoning_expected.clone();
        let generated = tokio::task::spawn_blocking(move || {
            let mut stream = LocalDeltaStream::new(stream_sender, streamed_flag, reasoning_flag);
            let result = runtime.generate(
                &request,
                budget.max_tokens,
                Some(turn_deadline),
                Some(std::time::Duration::from_secs(budget.no_progress_seconds)),
                budget.min_output,
                worker_cancel.as_ref(),
                &reasoning_expected,
                |delta| stream.push(delta),
            );
            stream.finish();
            result
        })
        .await
        .map_err(|error| format!("local inference worker failed: {error}"))??;
        let text = strip_reasoning(&generated.text);
        let truncated = generated.truncated;
        generated_total += generated.tokens;
        if generated.timed_out {
            let reason = match generated.timeout {
                Some(crate::local_model::TimeoutKind::NoProgress) => {
                    "produced no token within the idle window (a real stall)"
                }
                Some(crate::local_model::TimeoutKind::Deadline) => {
                    "hit the per-turn time limit while still generating (slow, not stalled)"
                }
                None => "was stopped by a watchdog",
            };
            return Err(format!(
                "The local model {reason} after {generated_total} tokens. A slow local model can over-run a turn on a long reasoning block; try a smaller prompt, a shorter request, or a faster build (CUDA/Metal)."
            ));
        }
        if generated_total >= budget.run_token_budget {
            return Err(format!(
                "The local model exceeded the {}-token budget for one run. Raise it with set_model_budget, or try a smaller prompt.",
                budget.run_token_budget
            ));
        }
        let (prose, tool_calls) = parse_local_tool_calls(&text);
        if !prose.is_empty() {
            final_text = prose.clone();
        }
        if tool_calls.is_empty() {
            // A `<tool_call>` block that did not parse is usually cut off at the
            // token cap, or invalid JSON. Tell the model and let it retry a
            // smaller call, rather than showing the raw block as the answer.
            if text.contains("<tool_call>") {
                if tool_call_retries < MAX_TOOL_CALL_RETRIES {
                    tool_call_retries += 1;
                    messages.push(crate::local_model::ChatMessage {
                        role: "assistant".into(),
                        content: text.clone(),
                    });
                    messages.push(crate::local_model::ChatMessage {
                        role: "user".into(),
                        content: LOCAL_TOOL_CALL_RETRY.into(),
                    });
                    continue;
                }
                return Err(
                    "the local model kept emitting a tool call that is not valid JSON; it may need a larger output limit or a smaller request"
                        .into(),
                );
            }
            let mut answer = if final_text.is_empty() {
                text.clone()
            } else {
                final_text.clone()
            };
            if claims_work(&answer) && !evidence.has_work() && nudges < MAX_CLAIM_NUDGES {
                nudges += 1;
                messages.push(crate::local_model::ChatMessage {
                    role: "assistant".into(),
                    content: text.clone(),
                });
                messages.push(crate::local_model::ChatMessage {
                    role: "user".into(),
                    content: CLAIM_NUDGE.into(),
                });
                continue;
            }
            if truncated {
                answer.push_str(&format!(
                    "\n\n[truncated] The local model reached this turn's {}-token output limit (the smaller of the run budget and the context room), so the reply is cut off. Raise it with set_model_budget, or use a larger context via RIGA_LOCAL_CONTEXT.",
                    budget.max_tokens
                ));
            }
            return Ok(AgentResult { output: answer });
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
                "task_id": task_id,
            });
            // Loop guards: cap the total, and stop an identical call the model
            // already made. Both turn "stuck globbing" into a clear stop instead
            // of burning every turn.
            tool_calls_total += 1;
            if tool_calls_total > LOCAL_MAX_TOOL_CALLS {
                return Err(format!(
                    "the local model made more than {LOCAL_MAX_TOOL_CALLS} tool calls without finishing; stopping so it does not loop. Ask a narrower question or use a larger model."
                ));
            }
            let repeats = {
                let entry = seen_calls
                    .entry(format!(
                        "{}\u{1}{}",
                        call.name,
                        serde_json::to_string(&call.arguments).unwrap_or_default()
                    ))
                    .or_insert(0usize);
                *entry += 1;
                *entry
            };
            if repeats > MAX_REPEATED_TOOL_CALLS {
                return Err(format!(
                    "the local model repeated the same `{}` call {repeats} times without finishing; stopping so it does not loop.",
                    call.name
                ));
            }
            if repeats > 1 {
                // The exact call already ran; its result is in the history. Feed a
                // reminder instead of executing it again, so the model has a
                // chance to answer before the cap above stops the run.
                trace_sender
                    .send(ToolTraceEvent::Started(payload.clone()))
                    .await
                    .map_err(|_| "tool lifecycle stream closed")?;
                let message = format!(
                    "You already called `{}` with these exact arguments; its result is in the history above. Do not repeat it — use that result to answer, or call a different tool.",
                    call.name
                );
                trace_sender
                    .send(ToolTraceEvent::Completed(ToolTrace {
                        call: payload.clone(),
                        name: call.name.clone(),
                        output: message.clone(),
                        ok: false,
                    }))
                    .await
                    .map_err(|_| "tool lifecycle stream closed")?;
                messages.push(crate::local_model::ChatMessage {
                    role: "tool".into(),
                    content: format!("<tool_response>\n{message}\n</tool_response>"),
                });
                continue;
            }
            trace_sender
                .send(ToolTraceEvent::Started(payload.clone()))
                .await
                .map_err(|_| "tool lifecycle stream closed")?;
            let disallowed =
                allowed_tools.is_some_and(|allowed| !allowed.iter().any(|name| name == &call.name));
            let result = if disallowed {
                Err(format!("`{}` is not available to this agent", call.name))
            } else {
                match authorize_tool(broker, &trace_sender, &call_id, &call.name, &call.arguments)
                    .await
                {
                    Err(message) => Err(message),
                    Ok(()) if call.name == "task" && is_subagent_dispatch(&call.arguments) => {
                        let agent = call
                            .arguments
                            .get("agent")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("subagent");
                        Box::pin(dispatch_subagent(
                            config,
                            workspace_root,
                            &call.arguments,
                            mcp_runtime,
                            &trace_sender,
                            depth,
                            broker,
                            Some(local_models),
                            evidence,
                        ))
                        .await
                        .map(|output| format!("[{agent} subagent result]\n{output}"))
                    }
                    Ok(()) => {
                        execute_tool(
                            workspace_root,
                            mcp_runtime,
                            &call.name,
                            call.arguments.clone(),
                            Some(ToolOutputStream {
                                call_id: call_id.clone(),
                                trace_sender: trace_sender.clone(),
                                graph: evidence.graph(),
                                budget: evidence.budget(),
                                tool_grants: evidence.tool_grants(),
                            }),
                        )
                        .await
                    }
                }
            };
            let (ok, output) = match result {
                Ok(output) => (true, output),
                Err(error) => (false, format!("tool error: {error}")),
            };
            if ok {
                consecutive_failures = 0;
            } else {
                consecutive_failures += 1;
            }
            evidence.record(&call.name, ok);
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
                    "<tool_response>\n{}\n</tool_response>",
                    crate::catalog::truncate_tool_result(
                        output.clone(),
                        crate::catalog::MAX_TOOL_RESULT_CHARS
                    )
                ),
            });
            // Stop a model that keeps re-sending a call that cannot succeed,
            // instead of letting it spend every remaining turn on the same error.
            if consecutive_failures >= MAX_CONSECUTIVE_TOOL_FAILURES {
                return Err(format!(
                    "the local model called `{}` {consecutive_failures} times in a row and each attempt failed; stopping so it does not loop. Last error: {output}",
                    call.name
                ));
            }
        }
        // A turn whose only calls record run state (plan/todos/graph) and that
        // produced prose is the model's answer: re-prompting it only makes a weak
        // model regenerate the same plan and loop. Finish here.
        if tool_calls
            .iter()
            .all(|call| is_record_only_tool(&call.name))
        {
            return Ok(AgentResult {
                output: final_text.clone(),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn call_responses_api(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    system_prompt: &str,
    allowed_tools: Option<&[String]>,
    prompt: &str,
    mcp_runtime: &McpRuntime,
    trace_sender: mpsc::Sender<ToolTraceEvent>,
    history: &[ConversationTurn],
    evidence: &RunEvidence,
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
    let current = format!("{}\n\nUser request:\n{}", system_prompt, prompt);
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
            // The old 1024 cut a long answer off mid-sentence. Reasoning tokens
            // count toward this too, so give a normal turn real room.
            "max_output_tokens": 4096,
            "reasoning": { "effort": effort },
            "tools": responses_tool_schemas_for(
                &mcp_runtime.tool_definitions().await,
                allowed_tools,
            ),
            // Stream so the assistant's prose reaches the client as it is
            // produced, the same way the chat-completions path does.
            "stream": true,
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
        if !status.is_success() {
            let raw = response.text().await.map_err(|e| e.to_string())?;
            return Err(format!(
                "provider Responses API returned HTTP {status}: {}",
                redact_body(&raw)
            ));
        }
        // A streaming provider answers with server-sent events whose terminal
        // `response.completed` frame carries the same object the non-streaming
        // call returns. A provider that ignores `stream` still returns one JSON
        // body, which is read the old way.
        let streaming = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
        let response: serde_json::Value = if streaming {
            read_responses_stream(response, &trace_sender, evidence).await?
        } else {
            let raw = response.text().await.map_err(|e| e.to_string())?;
            serde_json::from_str(&raw)
                .map_err(|e| format!("invalid provider Responses API response: {e}"))?
        };
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
                    graph: evidence.graph(),
                    budget: evidence.budget(),
                    tool_grants: evidence.tool_grants(),
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

/// Read a Responses API event stream to its terminal `response.completed` frame,
/// forwarding assistant text as it arrives.
///
/// The completed frame carries the whole response object — the same shape the
/// non-streaming call returns — so the tool loop downstream is unchanged. Text
/// deltas are emitted as they arrive so the prose is not held until the end.
async fn read_responses_stream(
    response: reqwest::Response,
    trace_sender: &mpsc::Sender<ToolTraceEvent>,
    evidence: &RunEvidence,
) -> Result<serde_json::Value, String> {
    let mut chunks = response.bytes_stream();
    let mut buffer = String::new();
    let mut completed: Option<serde_json::Value> = None;
    let mut failure: Option<String> = None;
    'stream: while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| format!("provider stream failed: {e}"))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(newline) = buffer.find('\n') {
            let line = buffer[..newline].trim_end_matches('\r').to_owned();
            buffer.drain(..=newline);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                break 'stream;
            }
            let value: serde_json::Value = serde_json::from_str(data)
                .map_err(|e| format!("invalid provider Responses stream frame: {e}"))?;
            match value.get("type").and_then(serde_json::Value::as_str) {
                Some("response.output_text.delta") => {
                    if let Some(delta) = value.get("delta").and_then(serde_json::Value::as_str)
                        && !delta.is_empty()
                    {
                        evidence.mark_streamed();
                        let _ = trace_sender
                            .send(ToolTraceEvent::Ui(RigaEvent::TextDelta {
                                delta: delta.to_owned(),
                            }))
                            .await;
                    }
                }
                Some("response.completed") => {
                    completed = value.get("response").cloned();
                    break 'stream;
                }
                Some("response.failed") | Some("response.error") => {
                    failure = value
                        .get("response")
                        .and_then(|frame| frame.get("error"))
                        .and_then(|error| error.get("message"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| {
                            value
                                .get("message")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        });
                }
                _ => {}
            }
        }
    }
    if let Some(message) = failure {
        return Err(format!("provider Responses API failed: {message}"));
    }
    completed.ok_or_else(|| "provider Responses stream ended without a response".to_owned())
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

fn responses_tool_schemas_for(
    mcp: &[rig_core::completion::ToolDefinition],
    allowed: Option<&[String]>,
) -> Vec<serde_json::Value> {
    let mut tools = responses_tool_schemas(mcp);
    if let Some(allowed) = allowed {
        tools.retain(|tool| {
            tool.get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| allowed.iter().any(|item| item == name))
        });
    }
    tools
}

/// Build a chat request from an already-resolved tool list, so a subagent can
/// run with a restricted subset.
/// One streaming tool call, assembled from the fragments a provider spreads
/// across several `delta` frames.
#[derive(Default)]
struct StreamedToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Read an SSE completion stream to its end, forwarding assistant text as it
/// arrives and reassembling streamed tool calls into a single message.
async fn read_streamed_message(
    response: reqwest::Response,
    trace_sender: &mpsc::Sender<ToolTraceEvent>,
    evidence: &RunEvidence,
) -> Result<serde_json::Value, String> {
    let mut chunks = response.bytes_stream();
    let mut buffer = String::new();
    let mut text = String::new();
    let mut fragments: std::collections::BTreeMap<usize, StreamedToolCall> =
        std::collections::BTreeMap::new();
    'stream: while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| format!("provider stream failed: {e}"))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(newline) = buffer.find('\n') {
            let line = buffer[..newline].trim_end_matches('\r').to_owned();
            buffer.drain(..=newline);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                break 'stream;
            }
            let value: serde_json::Value = serde_json::from_str(data)
                .map_err(|e| format!("invalid provider stream frame: {e}"))?;
            let Some(delta) = value.pointer("/choices/0/delta") else {
                continue;
            };
            if let Some(part) = absorb_stream_delta(delta, &mut text, &mut fragments) {
                // Mark the run as streamed so the final answer is not also sent
                // as one whole delta on top of what already reached the client.
                evidence.mark_streamed();
                let _ = trace_sender
                    .send(ToolTraceEvent::Ui(RigaEvent::TextDelta { delta: part }))
                    .await;
            }
        }
    }
    Ok(streamed_message(text, fragments))
}

/// Fold one streamed `delta` into the reply being assembled, returning the text
/// to forward onward when the frame carried assistant text.
fn absorb_stream_delta(
    delta: &serde_json::Value,
    text: &mut String,
    fragments: &mut std::collections::BTreeMap<usize, StreamedToolCall>,
) -> Option<String> {
    let mut emitted = None;
    if let Some(part) = delta.get("content").and_then(content_text)
        && !part.is_empty()
    {
        text.push_str(&part);
        emitted = Some(part);
    }
    if let Some(calls) = delta
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
    {
        for call in calls {
            let index = call
                .get("index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize;
            let entry = fragments.entry(index).or_default();
            if let Some(id) = call.get("id").and_then(serde_json::Value::as_str) {
                entry.id = id.to_owned();
            }
            if let Some(function) = call.get("function") {
                if let Some(name) = function.get("name").and_then(serde_json::Value::as_str) {
                    entry.name.push_str(name);
                }
                if let Some(args) = function
                    .get("arguments")
                    .and_then(serde_json::Value::as_str)
                {
                    entry.arguments.push_str(args);
                }
            }
        }
    }
    emitted
}

/// Build the assistant message a streamed turn describes. Tool-call fragments
/// are keyed by their provider index so several concurrent calls stay separate.
fn streamed_message(
    text: String,
    fragments: std::collections::BTreeMap<usize, StreamedToolCall>,
) -> serde_json::Value {
    let tool_calls: Vec<serde_json::Value> = fragments
        .values()
        .map(|fragment| {
            serde_json::json!({
                "id": fragment.id,
                "type": "function",
                "function": {
                    "name": fragment.name,
                    "arguments": fragment.arguments,
                },
            })
        })
        .collect();
    let content = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(text)
    };
    serde_json::json!({
        "role": "assistant",
        "content": content,
        "tool_calls": tool_calls,
    })
}

fn completion_request_body_with_tools(
    model: &str,
    messages: &[serde_json::Value],
    tools: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
        // A cap, not a target: a long file body needs room, short replies are
        // unaffected because generation stops at the end token.
        "max_completion_tokens": 4096,
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
            "Run a specialized subagent (action \"dispatch\" with `agent` and `prompt`), list the subagent profiles (action \"agents\"), or manage durable task records (list/inspect/create/update). action \"agents\" only lists; it runs nothing.",
            serde_json::json!({"type":"object","properties":{"action":{"type":"string","enum":["list","inspect","create","update","agents","agent_list","dispatch","agent","run"]},"agent":{"type":"string","enum":["explore","plan","build","review","scout","planner","executor","worker","reviewer"]},"prompt":{"type":"string"},"description":{"type":"string"},"name":{"type":"string"},"title":{"type":"string"},"status":{"type":"string"},"task_id":{"type":"string"},"node_id":{"type":"string"},"parent_task_id":{"type":"string"}},"required":[]}),
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
        function_schema(
            "update_graph",
            "Publish the current execution DAG. Each node needs a unique id, a known profile, a prompt, and optional dependency ids.",
            serde_json::json!({"type":"object","properties":{"title":{"type":"string"},"nodes":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"profile":{"type":"string","enum":["explore","plan","build","review"]},"description":{"type":"string"},"prompt":{"type":"string"},"depends_on":{"type":"array","items":{"type":"string"}}},"required":["id","profile","description","prompt"]}}},"required":["title","nodes"]}),
        ),
        function_schema(
            "set_model_budget",
            "Adjust the run's local-model budget: pick a capability tier (compact/middle/large), enlarge or shrink the current budget, or set exact limits. Takes effect on the next turn.",
            serde_json::json!({"type":"object","properties":{"tier":{"type":"string","enum":["compact","middle","large"]},"enlarge":{"type":"boolean"},"shrink":{"type":"boolean"},"max_tokens":{"type":"integer"},"min_output":{"type":"integer"},"run_token_budget":{"type":"integer"},"max_turns":{"type":"integer"},"turn_seconds":{"type":"integer"},"no_progress_seconds":{"type":"integer"}},"required":[]}),
        ),
        function_schema(
            "grant_tools",
            "Ask the user to grant a subagent extra tools for this run (for example `bash` for `plan`). Requires approval.",
            serde_json::json!({"type":"object","properties":{"profile":{"type":"string"},"tools":{"type":"array","items":{"type":"string"}},"reason":{"type":"string"}},"required":["profile","tools"]}),
        ),
        function_schema(
            "reset_tools",
            "Remove tools granted to a profile (or every profile when `profile` is omitted) for this run.",
            serde_json::json!({"type":"object","properties":{"profile":{"type":"string"}},"required":[]}),
        ),
    ]
}

/// The built-in tools as `ToolDefinition`s, so the local prompt can render them
/// in the model's native `<tools>` format. Without this the local prompt only
/// listed MCP tools and described the built-ins in prose, which is why models
/// like Hermes 3 improvised their own tool-call shape.
fn builtin_tool_definitions() -> Vec<rig_core::completion::ToolDefinition> {
    tool_schemas()
        .into_iter()
        .filter_map(|schema| {
            let function = schema.get("function")?;
            Some(rig_core::completion::ToolDefinition {
                name: function.get("name")?.as_str()?.to_owned(),
                description: function
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                parameters: function
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
            })
        })
        .collect()
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
fn coding_agent_system_prompt(workspace_root: &std::path::Path) -> String {
    let workspace = workspace_root;
    let mut prompt = String::from(
        "You are RIGA, a coding agent operating inside the configured workspace. \
For requests that create, modify, inspect, run, or validate software, use the available tools instead of only describing commands or code. \
Work in small observable steps: inspect first, then make the smallest change, then validate. \
Never claim a file or command succeeded unless a tool result confirms it. \
A listing, inspection, or plan is not a change: only a `write`, `bash`, or similar tool result proves a file exists or a command ran. \
Never put secrets, tokens, or API keys in tool arguments or in your replies; if a tool needs a credential, say so without inventing one.\n\n\
Tools:\n\
- `glob` takes a real glob pattern relative to the workspace (`src/**/*.ts`, `apps/*/package.json`). Dependency and build directories are already ignored.\n\
- `grep` searches file contents; `read` reads one file. Read a file before editing it.\n\
- `task` runs a subagent, but only when you call it with `action: \"dispatch\"`, an `agent`, and a `prompt`. The `action: \"agents\"` form only lists profiles — listing agents is not progress, so never report work done because you listed them. \
When the user addresses an agent with `@explore`, `@plan`, `@build`, or `@review`, dispatch it with `action: \"dispatch\"` and the matching `agent` rather than doing the work yourself when the profile's remit fits.\n\
- When coordinating multiple dependent agents or interpreting live/blocked RunDeck work, read `skills/rundeck/SKILL.md` first. It contains the graph workflow, exact dispatch forms, status meanings, and profile-specific prompt templates; do not ask read-only children to dispatch.\n\
- `update_plan` records the checklist you are working through and `update_todos` keeps your working list current. \
Call `update_plan` once right after exploring, then `update_todos` as work is discovered, started, finished, fails, or is dropped, instead of narrating progress in prose.\n\
- `update_graph` publishes a machine-readable DAG with unique node ids, known profiles, prompts, and `depends_on` edges. Use it when the work has dependencies; dispatch a graph node with `task` action `dispatch`, its `node_id`, and the matching profile. A node remains blocked until every dependency completes.\n\
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
        "For a request to create an app or service, create the files with `write` first and validate with `bash` \
after; never run an install command (`npm install`, `pip install`, …) before the manifest it reads \
(`package.json`, `requirements.txt`, …) exists, because it will fail and waste a turn. \
Do not spend turns enumerating subagents — listing profiles changes nothing.\n",
    );
    prompt.push_str(
        "Your edits land in an isolated git worktree for this session, not the user's working tree. \
When you finish, name the git branch (`riga/<session>`) and the workspace path above so the user can review and merge the result.\n",
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

#[cfg(test)]
mod tests {
    use super::{
        ClientMessage, ParsedToolCall, ProviderApi, ProviderConfig, ProviderKind, ServerMessage,
        builtin_tool_definitions, is_record_only_tool, local_tool_instructions,
        parse_local_tool_calls, resolve_run_provider, strip_reasoning,
    };
    use riga_kernel::events::RigaEvent;
    use tokio::sync::mpsc;

    #[test]
    fn reasoning_is_stripped_before_the_tool_call_is_parsed() {
        // The shape a Qwen3-style model emits: a reasoning block, then the call.
        let reply = "\u{3c}think\u{3e}\nThe user wants files.\n\u{3c}/think\u{3e}\n<tool_call>{\"name\": \"glob\", \"arguments\": {\"pattern\": \".\"}}</tool_call>";
        let stripped = strip_reasoning(reply);
        assert_eq!(
            stripped,
            "<tool_call>{\"name\": \"glob\", \"arguments\": {\"pattern\": \".\"}}</tool_call>"
        );
        let (prose, calls) = parse_local_tool_calls(&stripped);
        assert_eq!(prose, "");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "glob");
    }

    #[test]
    fn an_unterminated_reasoning_block_is_dropped() {
        assert_eq!(strip_reasoning("\u{3c}think\u{3e}\nstill thinking"), "");
    }

    #[test]
    fn a_template_opened_reasoning_block_is_stripped() {
        // Qwen3's template opens the block in the prompt, so the generated text
        // has no opening tag: it starts inside the block and only the close tag
        // marks where the answer begins.
        let reply = "The user wants files.\n\u{3c}/think\u{3e}\nHere is the answer.";
        assert_eq!(strip_reasoning(reply), "Here is the answer.");
    }

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
        assert_eq!(body["max_completion_tokens"], 4096);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["model"], "gpt-5-nano");
        // Streaming is on so a remote provider streams like a local one.
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn streamed_tool_call_fragments_reassemble_in_order() {
        let first: serde_json::Value = serde_json::from_str(
            r#"{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"write","arguments":"{\"path\":\"a.txt\","}}]}"#,
        )
        .unwrap();
        let second: serde_json::Value = serde_json::from_str(
            r#"{"tool_calls":[{"index":0,"function":{"arguments":"\"content\":\"hi\"}"}}]}"#,
        )
        .unwrap();
        let mut text = String::new();
        let mut fragments = std::collections::BTreeMap::new();
        assert!(super::absorb_stream_delta(&first, &mut text, &mut fragments).is_none());
        assert!(super::absorb_stream_delta(&second, &mut text, &mut fragments).is_none());
        let message = super::streamed_message(text, fragments);
        assert_eq!(message["tool_calls"][0]["id"], "call_1");
        assert_eq!(message["tool_calls"][0]["function"]["name"], "write");
        assert_eq!(
            message["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"a.txt","content":"hi"}"#
        );
        // A tool-call-only turn has no text.
        assert!(message["content"].is_null());
    }

    #[test]
    fn streamed_text_fragments_accumulate_and_forward() {
        let mut text = String::new();
        let mut fragments = std::collections::BTreeMap::new();
        let first = serde_json::json!({"content": "Hel"});
        let second = serde_json::json!({"content": "lo"});
        assert_eq!(
            super::absorb_stream_delta(&first, &mut text, &mut fragments),
            Some("Hel".to_owned())
        );
        assert_eq!(
            super::absorb_stream_delta(&second, &mut text, &mut fragments),
            Some("lo".to_owned())
        );
        assert_eq!(text, "Hello");
        assert_eq!(super::streamed_message(text, fragments)["content"], "Hello");
    }

    #[test]
    fn coding_agent_prompt_requires_tools_for_implementation_requests() {
        let prompt = super::coding_agent_system_prompt(std::path::Path::new("/tmp"));
        assert!(prompt.contains("use the available tools"));
        assert!(prompt.contains("Never claim a file or command succeeded"));
    }

    #[test]
    fn coding_agent_prompt_states_capabilities_and_agent_mentions() {
        // The model knows writes/shell pause for approval and that `@agent`
        // addresses a dispatchable subagent.
        let prompt = super::coding_agent_system_prompt(std::path::Path::new("/tmp"));
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
        assert!(dispatch.contains("\"agent\": \"build\""));
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
    fn local_models_may_use_dispatch_as_a_task_action_alias() {
        let (_, calls) = parse_local_tool_calls(
            r#"<tool_call>{"name":"dispatch","arguments":{"agent":"explore","prompt":"Study this repo"}}</tool_call>"#,
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "task");
        assert_eq!(calls[0].arguments["action"], "dispatch");
        assert_eq!(calls[0].arguments["agent"], "explore");
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
        // An object whose keys match no tool schema cannot be dispatched, and
        // guessing a tool would be worse than ignoring it.
        let (_, calls) = parse_local_tool_calls("<tool_call>{\"arguments\":{}}</tool_call>");
        assert!(calls.is_empty());
        let (_, calls) = parse_local_tool_calls("<tool_call>{\"unrecognized\":\"x\"}</tool_call>");
        assert!(calls.is_empty());
    }

    #[test]
    fn a_fenced_json_dispatch_is_parsed_as_a_task_call() {
        // Hermes-3 shape: the arguments alone, inside a ```json fence, with no
        // `<tool_call>` wrapper and no `name`.
        let (prose, calls) = parse_local_tool_calls(
            "First, I'll dispatch the plan agent:\n\n```json\n{\"action\": \"dispatch\", \"agent\": \"plan\", \"prompt\": \"plan it\"}\n```\n\nThen I'll wait.",
        );
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].name, "task");
        assert_eq!(calls[0].arguments["agent"], "plan");
        assert_eq!(calls[0].arguments["action"], "dispatch");
        assert!(prose.contains("Then I'll wait"), "{prose}");
        assert!(
            !prose.contains("\"action\""),
            "the fence is consumed: {prose}"
        );
    }

    #[test]
    fn a_fenced_named_call_or_bare_shape_is_parsed() {
        let (_, calls) = parse_local_tool_calls(
            "```json\n{\"name\": \"read\", \"arguments\": {\"path\": \"a.txt\"}}\n```",
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read");

        // No name: the argument keys name the tool.
        let (_, calls) = parse_local_tool_calls("```json\n{\"command\": \"ls -la\"}\n```");
        assert_eq!(calls[0].name, "bash");
        assert_eq!(calls[0].arguments["command"], "ls -la");
        let (_, calls) = parse_local_tool_calls("```json\n{\"path\": \"a.txt\"}\n```");
        assert_eq!(calls[0].name, "read");
        let (_, calls) = parse_local_tool_calls("```json\n{\"pattern\": \"src/**\"}\n```");
        assert_eq!(calls[0].name, "glob");
    }

    #[test]
    fn a_non_tool_fence_stays_prose() {
        let (prose, calls) = parse_local_tool_calls(
            "Run this:\n\n```bash\nls -la\n```\n\nConfig:\n\n```json\n{\"key\": \"value\"}\n```\n",
        );
        assert!(calls.is_empty(), "{calls:?}");
        assert!(prose.contains("ls -la"), "{prose}");
        assert!(prose.contains("\"key\""), "{prose}");
    }

    #[test]
    fn a_loose_key_value_task_call_is_parsed() {
        // Hermes-3 shape 2: a `text` fence with the loose key:value form.
        let (prose, calls) = parse_local_tool_calls(
            "I'll dispatch the plan agent:\n\n```text\ntask action:dispatch agent:plan prompt:Plan steps to find the top 10 largest files in the workspace.\n```\n\nThen build.",
        );
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].name, "task");
        assert_eq!(calls[0].arguments["action"], "dispatch");
        assert_eq!(calls[0].arguments["agent"], "plan");
        assert_eq!(
            calls[0].arguments["prompt"],
            "Plan steps to find the top 10 largest files in the workspace."
        );
        assert!(prose.contains("Then build"), "{prose}");
        assert!(!prose.contains("agent:plan"), "{prose}");
    }

    #[test]
    fn a_loose_key_value_call_works_without_a_fence() {
        let (_, calls) = parse_local_tool_calls("bash command:ls -la /tmp");
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].name, "bash");
        assert_eq!(calls[0].arguments["command"], "ls -la /tmp");
    }

    #[test]
    fn loose_parsing_leaves_prose_alone() {
        // No recognized key, or the token after the name is not a key.
        let (prose, calls) = parse_local_tool_calls("read the file carefully");
        assert!(calls.is_empty(), "{calls:?}");
        assert_eq!(prose, "read the file carefully");
        let (prose, calls) = parse_local_tool_calls("The task: find the largest files");
        assert!(calls.is_empty(), "{calls:?}");
        assert!(prose.contains("find the largest files"), "{prose}");
    }

    #[test]
    fn a_prose_wrapped_tool_call_object_is_salvaged() {
        // A model that wraps the object in prose still yields a call.
        let (_, calls) = parse_local_tool_calls(
            "<tool_call>Sure: {\"name\": \"read\", \"arguments\": {\"path\": \"a.txt\"}} ok</tool_call>",
        );
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[0].arguments["path"], "a.txt");
    }

    #[test]
    fn arguments_only_objects_are_matched_to_a_tool_by_schema() {
        // The wrapper-less shape Qwen2.5 Coder emits: the arguments alone, no
        // `name`. Each is matched to the tool whose schema it fits.
        let cases = [
            (
                r#"{"active_index":0,"steps":[{"id":"1","label":"find files"}],"title":"Find files"}"#,
                "update_plan",
            ),
            (
                r#"{"content":"plan text","path":"src/planning/plan.txt"}"#,
                "write",
            ),
            (r#"{"command":"ls -la"}"#, "bash"),
            (r#"{"pattern":"src/**"}"#, "glob"),
            (r#"{"query":"needle"}"#, "grep"),
            (r#"{"path":"a.txt"}"#, "read"),
            (
                r#"{"action":"dispatch","agent":"plan","prompt":"plan it"}"#,
                "task",
            ),
        ];
        for (raw, expected) in cases {
            let (_, calls) = parse_local_tool_calls(&format!("```json\n{raw}\n```"));
            assert_eq!(calls.len(), 1, "{raw} -> {calls:?}");
            assert_eq!(calls[0].name, expected, "{raw}");
        }
        // A JSON example in prose matches no schema and stays prose.
        let (prose, calls) = parse_local_tool_calls("```json\n{\"key\": \"value\"}\n```");
        assert!(calls.is_empty(), "{calls:?}");
        assert!(prose.contains("\"key\""), "{prose}");
    }

    #[test]
    fn record_only_tools_are_recognized() {
        // A turn that only records run state finishes the loop.
        for name in ["update_plan", "update_todos", "update_graph"] {
            assert!(is_record_only_tool(name), "{name}");
        }
        // Anything that gathers information or acts keeps the loop going.
        for name in ["read", "bash", "task", "set_model_budget", "grant_tools"] {
            assert!(!is_record_only_tool(name), "{name}");
        }
    }

    #[test]
    fn builtin_tool_definitions_cover_the_model_facing_tools() {
        let defs = builtin_tool_definitions();
        let names: Vec<&str> = defs
            .iter()
            .map(|definition| definition.name.as_str())
            .collect();
        for expected in [
            "read",
            "write",
            "bash",
            "task",
            "update_plan",
            "set_model_budget",
        ] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }
        for definition in &defs {
            assert!(
                !definition.description.is_empty(),
                "{} has no description",
                definition.name
            );
            assert!(
                definition.parameters.is_object(),
                "{} has no object schema",
                definition.name
            );
        }
    }

    #[test]
    fn local_tool_instructions_render_the_native_tools_block() {
        let defs = vec![rig_core::completion::ToolDefinition {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        }];
        let text = local_tool_instructions(&defs);
        assert!(text.contains("# Tools"), "{text}");
        assert!(text.contains("<tools>"), "{text}");
        assert!(text.contains("</tools>"), "{text}");
        assert!(text.contains("\"name\":\"read\""), "{text}");
        assert!(text.contains("\"parameters\""), "{text}");
        assert!(text.contains("<tool_call>"), "{text}");
    }

    #[tokio::test]
    async fn update_plan_emits_a_plan_frame_and_confirms() {
        let (sender, mut receiver) = mpsc::channel(4);
        let stream = super::ToolOutputStream {
            call_id: "call-1".into(),
            trace_sender: sender,
            graph: std::sync::Arc::new(tokio::sync::Mutex::new(super::GraphRuntime::default())),
            budget: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::local_model::LocalBudget::default(),
            )),
            tool_grants: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
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
            graph: std::sync::Arc::new(tokio::sync::Mutex::new(super::GraphRuntime::default())),
            budget: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::local_model::LocalBudget::default(),
            )),
            tool_grants: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
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
    async fn update_graph_validates_and_emits_readiness_events() {
        let (sender, mut receiver) = mpsc::channel(16);
        let stream = super::ToolOutputStream {
            call_id: "call-graph".into(),
            trace_sender: sender,
            graph: std::sync::Arc::new(tokio::sync::Mutex::new(super::GraphRuntime::default())),
            budget: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::local_model::LocalBudget::default(),
            )),
            tool_grants: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        };
        let result = super::execute_tool(
            std::path::Path::new("."),
            &crate::mcp::McpRuntime::new(),
            "update_graph",
            serde_json::json!({
                "title": "service",
                "nodes": [
                    {"id": "runtime", "profile": "explore", "description": "inspect", "prompt": "inspect"},
                    {"id": "build", "profile": "build", "description": "implement", "prompt": "implement", "depends_on": ["runtime"]}
                ]
            }),
            Some(stream),
        ).await.expect("graph validates");
        assert!(result.contains("2 node(s), 1 ready"), "{result}");
        assert!(matches!(
            receiver.recv().await.unwrap(),
            super::ToolTraceEvent::Ui(RigaEvent::GraphUpdated { .. })
        ));
        assert!(
            matches!(receiver.recv().await.unwrap(), super::ToolTraceEvent::Ui(RigaEvent::TaskDependencyAdded { task_id, depends_on }) if task_id == "build" && depends_on == "runtime")
        );
        assert!(
            matches!(receiver.recv().await.unwrap(), super::ToolTraceEvent::Ui(RigaEvent::TaskRunnable { task_id }) if task_id == "runtime")
        );
        assert!(
            matches!(receiver.recv().await.unwrap(), super::ToolTraceEvent::Ui(RigaEvent::TaskBlocked { task_id, blocked_by }) if task_id == "build" && blocked_by == vec!["runtime"])
        );
    }

    /// A `ToolOutputStream` for tool tests, sharing the given budget and grants.
    fn tool_test_stream(
        budget: std::sync::Arc<tokio::sync::Mutex<crate::local_model::LocalBudget>>,
        tool_grants: std::sync::Arc<
            tokio::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
        >,
    ) -> super::ToolOutputStream {
        let (sender, _receiver) = mpsc::channel(4);
        super::ToolOutputStream {
            call_id: "call-test".into(),
            trace_sender: sender,
            graph: std::sync::Arc::new(tokio::sync::Mutex::new(super::GraphRuntime::default())),
            budget,
            tool_grants,
        }
    }

    #[tokio::test]
    async fn set_model_budget_updates_the_run_budget() {
        let budget = std::sync::Arc::new(tokio::sync::Mutex::new(
            crate::local_model::LocalBudget::default(),
        ));
        let grants = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let stream = tool_test_stream(budget.clone(), grants);
        let result = super::execute_tool(
            std::path::Path::new("."),
            &crate::mcp::McpRuntime::new(),
            "set_model_budget",
            serde_json::json!({"tier": "compact"}),
            Some(stream),
        )
        .await
        .expect("budget tool executes");
        assert!(result.contains("Model budget updated"), "{result}");
        assert_eq!(
            *budget.lock().await,
            crate::local_model::Capability::Compact.budget()
        );
    }

    #[tokio::test]
    async fn grant_tools_extends_and_reset_tools_clears_grants() {
        let budget = std::sync::Arc::new(tokio::sync::Mutex::new(
            crate::local_model::LocalBudget::default(),
        ));
        let grants = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let stream = tool_test_stream(budget.clone(), grants.clone());
        let result = super::execute_tool(
            std::path::Path::new("."),
            &crate::mcp::McpRuntime::new(),
            "grant_tools",
            serde_json::json!({"profile": "plan", "tools": ["webfetch"]}),
            Some(stream),
        )
        .await
        .expect("grant tool executes");
        assert!(result.contains("Granted to `plan`"), "{result}");
        assert_eq!(
            grants.lock().await.get("plan").cloned(),
            Some(vec!["webfetch".to_owned()])
        );

        let stream = tool_test_stream(budget, grants.clone());
        let result = super::execute_tool(
            std::path::Path::new("."),
            &crate::mcp::McpRuntime::new(),
            "reset_tools",
            serde_json::json!({}),
            Some(stream),
        )
        .await
        .expect("reset tool executes");
        assert!(result.contains("Cleared all tool grants"), "{result}");
        assert!(grants.lock().await.is_empty());
    }

    #[test]
    fn run_grants_extend_a_profiles_allowed_tools() {
        let plan = crate::catalog::find_agent_profile("plan").expect("plan profile");
        let base = super::allowed_tools_for(&plan, None);
        assert!(!base.iter().any(|tool| tool == "webfetch"));
        let granted = super::allowed_tools_for(&plan, Some(&["webfetch".to_owned()]));
        assert!(granted.iter().any(|tool| tool == "webfetch"));
    }

    #[tokio::test]
    async fn graph_dispatch_rejects_incomplete_dependencies() {
        let evidence = super::RunEvidence::default();
        {
            let graph = evidence.graph();
            let mut runtime = graph.lock().await;
            runtime.graph = Some(riga_kernel::task::Graph {
                title: "service".into(),
                nodes: vec![
                    riga_kernel::task::GraphNode {
                        id: "runtime".into(),
                        profile: "explore".into(),
                        description: "inspect".into(),
                        prompt: "inspect".into(),
                        depends_on: vec![],
                    },
                    riga_kernel::task::GraphNode {
                        id: "build".into(),
                        profile: "build".into(),
                        description: "implement".into(),
                        prompt: "implement".into(),
                        depends_on: vec!["runtime".into()],
                    },
                ],
            });
            runtime
                .states
                .insert("runtime".into(), riga_kernel::task::TaskState::Pending);
            runtime
                .states
                .insert("build".into(), riga_kernel::task::TaskState::Pending);
        }
        let (sender, mut receiver) = mpsc::channel(4);
        let config = super::ProviderConfig {
            endpoint: "http://127.0.0.1:1".into(),
            api_key: String::new(),
            model: "test".into(),
            reasoning_effort: "low".into(),
            kind: super::ProviderKind::Remote,
            api: super::ProviderApi::Chat,
            subagent_model: None,
        };
        let error = super::dispatch_subagent(
            &config,
            std::path::Path::new("."),
            &serde_json::json!({"agent": "build", "node_id": "build", "prompt": "implement"}),
            &crate::mcp::McpRuntime::new(),
            &sender,
            0,
            &super::ApprovalBroker::default(),
            None,
            &evidence,
        )
        .await
        .expect_err("blocked node must not start");
        assert!(
            error.contains("blocked by incomplete dependencies"),
            "{error}"
        );
        assert!(
            matches!(receiver.recv().await.unwrap(), super::ToolTraceEvent::Ui(RigaEvent::TaskBlocked { task_id, blocked_by }) if task_id == "build" && blocked_by == vec!["runtime"])
        );
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
        assert!(super::is_subagent_dispatch(
            &serde_json::json!({"action": "run", "agent": "review", "prompt": "check it"})
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
    fn graph_events_are_durable_but_reasoning_deltas_are_ephemeral() {
        assert!(!super::is_ephemeral(&RigaEvent::GraphUpdated {
            graph: Box::new(riga_kernel::task::Graph {
                title: "x".into(),
                nodes: vec![]
            }),
        }));
        assert!(!super::is_ephemeral(&RigaEvent::TaskRunnable {
            task_id: "a".into()
        }));
        assert!(super::is_ephemeral(&RigaEvent::ReasoningDelta {
            delta: "thinking".into()
        }));
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
        // A mention is honoured anywhere in the sentence, with the surrounding
        // text kept as the task: users write "Build X ... @build make sure ...".
        assert_eq!(
            super::leading_agent_mention("Build X @build make sure"),
            Some(("build".to_owned(), "Build X make sure".to_owned()))
        );
        // Matching is case-insensitive.
        assert_eq!(
            super::leading_agent_mention("please @Build this").map(|(agent, _)| agent),
            Some("build".to_owned())
        );
        // A mention with no task, or an `@` that names no profile (an email),
        // is left to the orchestrator.
        assert!(super::leading_agent_mention("@explore").is_none());
        assert!(super::leading_agent_mention("@unknown do a thing").is_none());
        assert!(super::leading_agent_mention("mail me at user@example.com").is_none());
    }

    #[test]
    fn read_only_profiles_cannot_write_or_dispatch() {
        // `explore` and `review` may run approval-gated shell commands for
        // reconnaissance; neither can write or dispatch.
        for agent in ["explore", "review"] {
            let profile = crate::catalog::find_agent_profile(agent).expect("read-only profile");
            let allowed = super::allowed_tools_for(&profile, None);
            for forbidden in ["write", "task", "shell"] {
                assert!(
                    !allowed.iter().any(|tool| tool == forbidden),
                    "{agent} must not be allowed `{forbidden}`: {allowed:?}"
                );
            }
            for expected in ["read", "glob", "grep", "bash", "skill"] {
                assert!(
                    allowed.iter().any(|tool| tool == expected),
                    "{agent} should be allowed `{expected}`: {allowed:?}"
                );
            }
        }
        // `plan` plans: it reads and searches, but must not run commands or it
        // loops on the plan's own shell steps instead of returning a plan.
        let plan = crate::catalog::find_agent_profile("plan").expect("plan profile");
        let allowed = super::allowed_tools_for(&plan, None);
        for forbidden in ["write", "task", "shell", "bash"] {
            assert!(
                !allowed.iter().any(|tool| tool == forbidden),
                "plan must not be allowed `{forbidden}`: {allowed:?}"
            );
        }
        for expected in ["read", "glob", "grep", "skill"] {
            assert!(
                allowed.iter().any(|tool| tool == expected),
                "plan should be allowed `{expected}`: {allowed:?}"
            );
        }
    }

    #[test]
    fn build_profile_may_write_and_dispatch() {
        let build = crate::catalog::find_agent_profile("build").expect("build profile");
        let allowed = super::allowed_tools_for(&build, None);
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
        let prompt = super::subagent_system_prompt(&review, std::path::Path::new("/tmp/ws"));
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

    #[test]
    fn work_claims_are_detected_but_ordinary_explanations_are_not() {
        // The reported hallucination: this exact phrasing, with no write.
        assert!(super::claims_work(
            "An Express web service has been created and a React app has been served from it."
        ));
        assert!(super::claims_work(
            "I created the app and started the server."
        ));
        assert!(super::claims_work("Files changed: 3"));
        // Explanation is not a claim; the guard must not fire on a Q&A run.
        assert!(!super::claims_work(
            "The function creates a file when it is called."
        ));
        assert!(!super::claims_work("We will create the app next."));
        assert!(!super::claims_work("Here is how to write a Node server."));
    }

    #[test]
    fn only_write_and_shell_count_as_work() {
        assert!(super::is_mutating_tool("write"));
        assert!(super::is_mutating_tool("bash"));
        assert!(super::is_mutating_tool("shell"));
        for read_only in ["read", "glob", "grep", "web", "task"] {
            assert!(
                !super::is_mutating_tool(read_only),
                "`{read_only}` must not count as work"
            );
        }
    }

    #[test]
    fn evidence_only_records_successful_mutations() {
        let evidence = super::RunEvidence::default();
        assert!(!evidence.has_work());
        // A failed write is not evidence of work.
        evidence.record("write", false);
        assert!(!evidence.has_work());
        // A read is not work either.
        evidence.record("read", true);
        assert!(!evidence.has_work());
        evidence.record("bash", true);
        assert!(evidence.has_work());
    }

    #[test]
    fn cancellation_reaches_every_clone_of_the_evidence() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let cancel = Arc::new(AtomicBool::new(false));
        let parent = super::RunEvidence::with_cancel(cancel.clone());
        // A subagent gets a clone; cancelling the run must reach it too.
        let child = parent.clone();
        assert!(!child.is_cancelled());
        cancel.store(true, Ordering::Relaxed);
        assert!(parent.is_cancelled());
        assert!(child.is_cancelled());
    }

    #[test]
    fn only_completed_and_failed_events_end_a_follow() {
        assert!(super::is_terminal(&RigaEvent::RunCompleted {
            output: String::new()
        }));
        assert!(super::is_terminal(&RigaEvent::RunFailed {
            message: String::new()
        }));
        assert!(!super::is_terminal(&RigaEvent::RunStarted));
    }

    #[test]
    fn run_channel_broadcasts_sequential_events() {
        let (events, mut receiver) = tokio::sync::broadcast::channel(8);
        let mut channel = super::RunChannel::without_journal("run-1", "session-1", events);
        channel.emit(RigaEvent::RunStarted);
        channel.emit_trace(super::ToolTraceEvent::Ui(RigaEvent::TextDelta {
            delta: "hi".into(),
        }));
        channel.emit(RigaEvent::RunCompleted {
            output: "hi".into(),
        });
        let mut got = Vec::new();
        while let Ok(envelope) = receiver.try_recv() {
            got.push((envelope.sequence, envelope.run_id, envelope.session_id));
        }
        // Sequence starts at 1 and advances by one, which is what the client's
        // dedupe cursor relies on.
        assert_eq!(
            got,
            vec![
                (1, "run-1".to_owned(), "session-1".to_owned()),
                (2, "run-1".to_owned(), "session-1".to_owned()),
                (3, "run-1".to_owned(), "session-1".to_owned()),
            ]
        );
    }

    #[test]
    fn path_arguments_accept_the_aliases_small_models_emit() {
        assert_eq!(
            super::path_arg(&serde_json::json!({"path": "a"})),
            Some("a")
        );
        assert_eq!(
            super::path_arg(&serde_json::json!({"file": "b"})),
            Some("b")
        );
        assert_eq!(
            super::path_arg(&serde_json::json!({"filename": "c"})),
            Some("c")
        );
        assert_eq!(
            super::path_arg(&serde_json::json!({"file_path": "d"})),
            Some("d")
        );
        // Canonical key wins when both are present.
        assert_eq!(
            super::path_arg(&serde_json::json!({"path": "a", "file": "b"})),
            Some("a")
        );
        assert_eq!(super::path_arg(&serde_json::json!({})), None);
    }

    #[test]
    fn content_arguments_accept_the_aliases_small_models_emit() {
        // Phi-4 emits `text` where the schema says `content`.
        assert_eq!(
            super::content_arg(&serde_json::json!({"content": "a"})),
            Some("a")
        );
        assert_eq!(
            super::content_arg(&serde_json::json!({"text": "b"})),
            Some("b")
        );
        assert_eq!(
            super::content_arg(&serde_json::json!({"body": "c"})),
            Some("c")
        );
        assert_eq!(
            super::content_arg(&serde_json::json!({"contents": "d"})),
            Some("d")
        );
        // Canonical key wins when both are present.
        assert_eq!(
            super::content_arg(&serde_json::json!({"content": "a", "text": "b"})),
            Some("a")
        );
        assert_eq!(super::content_arg(&serde_json::json!({})), None);
    }

    #[test]
    fn argument_errors_name_the_expected_and_received_keys() {
        let error = super::argument_error(
            "write",
            "{\"path\": \"<file>\", \"content\": \"<text>\"} (one file per call)",
            &serde_json::json!({"files": ["a", "b"]}),
        );
        // The model is told the keys it should use and the keys it did use.
        assert!(error.contains("path"), "{error}");
        assert!(error.contains("content"), "{error}");
        assert!(error.contains("files"), "{error}");
        assert!(error.contains("one file per call"), "{error}");
    }

    #[test]
    fn install_preflight_requires_a_manifest() {
        let dir = std::env::temp_dir().join(format!("riga-preflight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        // No manifest anywhere: refused with a message that names the fix.
        let error = super::install_preflight(&dir, "npm install").expect("refused");
        assert!(error.contains("package.json"), "{error}");
        assert!(
            super::install_preflight(&dir, "cd app && npm install").is_some(),
            "a missing cd target has no manifest either"
        );
        // A non-install command is never blocked.
        assert!(super::install_preflight(&dir, "node app.js").is_none());
        // The manifest in the cd target satisfies it.
        std::fs::write(dir.join("app").join("package.json"), "{}").unwrap();
        assert!(super::install_preflight(&dir, "cd app && npm install").is_none());
        // Python reads a different manifest, so a package.json does not satisfy
        // `pip install`.
        assert!(super::install_preflight(&dir, "pip install flask").is_some());
        std::fs::write(dir.join("requirements.txt"), "flask").unwrap();
        assert!(super::install_preflight(&dir, "pip install flask").is_none());
        // A manifest in the workspace root satisfies npm.
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        assert!(super::install_preflight(&dir, "npm install").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tool_calls_survive_a_multiline_file_body() {
        // The common failure: a file body pasted with real newlines and tabs
        // inside the JSON string. It is invalid JSON, but must still parse.
        let raw = "{\"name\": \"write\", \"arguments\": {\"path\": \"a.txt\", \"content\": \"line1\nline2\ttab\"}}";
        assert!(serde_json::from_str::<serde_json::Value>(raw).is_err());
        let call = super::parse_one_local_tool_call(raw).expect("repaired");
        assert_eq!(call.name, "write");
        assert_eq!(call.arguments["path"], "a.txt");
        assert_eq!(call.arguments["content"], "line1\nline2\ttab");
    }

    #[test]
    fn escaping_leaves_valid_json_untouched() {
        let raw = r#"{"content": "a \"quoted\" word\nb"}"#;
        assert_eq!(super::escape_json_control_chars(raw), raw);
    }

    #[tokio::test]
    async fn write_accepts_text_as_content_end_to_end() {
        // The exact call Phi-4 emitted: `text` instead of `content`.
        let dir = std::env::temp_dir().join(format!("riga-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = crate::mcp::McpRuntime::new();
        let result = super::execute_tool(
            &dir,
            &runtime,
            "write",
            serde_json::json!({"path": "essay.txt", "text": "sunny"}),
            None,
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("essay.txt")).unwrap(),
            "sunny"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_stream_forwards_prose_and_keeps_tool_calls_out() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let drain = |mut receiver: mpsc::Receiver<super::ToolTraceEvent>| {
            let (mut text, mut reasoning) = (String::new(), String::new());
            while let Ok(event) = receiver.try_recv() {
                match event {
                    super::ToolTraceEvent::Ui(RigaEvent::TextDelta { delta }) => {
                        text.push_str(&delta)
                    }
                    super::ToolTraceEvent::Ui(RigaEvent::ReasoningDelta { delta }) => {
                        reasoning.push_str(&delta)
                    }
                    _ => {}
                }
            }
            (text, reasoning)
        };
        let no_reasoning = || Arc::new(AtomicBool::new(false));

        // Prose streams, including a short turn held back until it settles.
        let (sender, receiver) = mpsc::channel(16);
        let flag = Arc::new(AtomicBool::new(false));
        let mut stream = super::LocalDeltaStream::new(sender, flag.clone(), no_reasoning());
        stream.push("The answer is ");
        stream.push("42.");
        stream.finish();
        assert_eq!(drain(receiver).0, "The answer is 42.");
        assert!(flag.load(Ordering::Relaxed), "streaming must be recorded");

        // A tool-call turn is not shown at all, and must not mark the run as
        // streamed (the tool result is what carries it).
        let (sender, receiver) = mpsc::channel(16);
        let flag = Arc::new(AtomicBool::new(false));
        let mut stream = super::LocalDeltaStream::new(sender, flag.clone(), no_reasoning());
        stream.push("<tool_call>{\"name\":\"read\",");
        stream.push("\"arguments\":{\"path\":\"a\"}}</tool_call>");
        stream.finish();
        assert_eq!(drain(receiver).0, "");
        assert!(!flag.load(Ordering::Relaxed));

        // Reasoning streams as ReasoningDelta (never prose), and the tool call
        // that follows stays out of the reply. The template opened the block, so
        // the output carries no ` thinking` tag of its own.
        let (sender, receiver) = mpsc::channel(16);
        let flag = Arc::new(AtomicBool::new(false));
        let mut stream =
            super::LocalDeltaStream::new(sender, flag.clone(), Arc::new(AtomicBool::new(true)));
        stream.push("The user wants files.");
        stream.push("\u{3c}/think\u{3e}");
        stream.push("<tool_call>{\"name\":\"glob\",\"arguments\":{\"pattern\":\".\"}}</tool_call>");
        stream.finish();
        let (text, reasoning) = drain(receiver);
        assert_eq!(reasoning, "The user wants files.");
        assert_eq!(text, "");
        assert!(!flag.load(Ordering::Relaxed), "reasoning is not reply text");

        // A newline inside the reasoning must not end the block: the whole
        // reasoning stays one ReasoningDelta stream, and only the text after the
        // close tag is prose.
        let (sender, receiver) = mpsc::channel(16);
        let flag = Arc::new(AtomicBool::new(false));
        let mut stream =
            super::LocalDeltaStream::new(sender, flag.clone(), Arc::new(AtomicBool::new(true)));
        stream.push("Okay, list files.\n");
        stream.push("First, I need to list.");
        stream.push("\u{3c}/think\u{3e}");
        stream.push("The answer is 42.");
        stream.finish();
        let (text, reasoning) = drain(receiver);
        assert_eq!(reasoning, "Okay, list files.\nFirst, I need to list.");
        assert_eq!(text, "The answer is 42.");

        // The model repeating the template's opening tag is not reasoning text.
        let (sender, receiver) = mpsc::channel(16);
        let flag = Arc::new(AtomicBool::new(false));
        let mut stream =
            super::LocalDeltaStream::new(sender, flag.clone(), Arc::new(AtomicBool::new(true)));
        stream.push("\u{3c}think\u{3e}\nReasoning.");
        stream.push("\u{3c}/think\u{3e}");
        stream.finish();
        let (text, reasoning) = drain(receiver);
        assert_eq!(reasoning, "Reasoning.");
        assert_eq!(text, "");
    }
}
