#![allow(dead_code)]

use std::collections::BTreeMap;

use riga_kernel::{
    PROTOCOL_VERSION,
    events::{EvidenceNode, KnowledgeNode, RigaEvent, RigaEventEnvelope},
    state::Session,
    task::{Graph, Plan, TaskRecord, TaskState, TodoList},
};
use riga_server::ws::ActiveRun;

const MAX_TRANSCRIPT_ITEMS: usize = 2_000;
const MAX_TOOL_OUTPUT_CHARS: usize = 100_000;
const MAX_REASONING_CHARS: usize = 80_000;
const MAX_EVIDENCE: usize = 1_000;
const MAX_KNOWLEDGE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunStatus {
    #[default]
    Unknown,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptItem {
    AssistantText(String),
    AssistantReasoning(String),
    ToolCall {
        call: serde_json::Value,
    },
    ToolOutput {
        call_id: String,
        output: String,
    },
    ToolResult {
        result: serde_json::Value,
    },
    TaskResult {
        task_id: String,
        ok: bool,
        result: String,
    },
    Approval {
        approval_id: String,
        tool: String,
        summary: String,
    },
    System(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskView {
    pub record: Option<TaskRecord>,
    pub state: TaskState,
    pub elapsed_ms: u64,
    pub result: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalView {
    pub approval_id: String,
    pub task_id: String,
    pub tool: String,
    pub summary: String,
    pub resolved: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunView {
    pub run_id: String,
    pub session_id: String,
    pub status: RunStatus,
    pub last_sequence: u64,
    pub needs_replay: bool,
    pub output: String,
    pub reasoning: String,
    pub transcript: Vec<TranscriptItem>,
    pub plan: Option<Plan>,
    pub todos: Option<TodoList>,
    pub graph: Option<Graph>,
    pub tasks: BTreeMap<String, TaskView>,
    pub evidence: Vec<EvidenceNode>,
    pub knowledge: Vec<KnowledgeNode>,
    pub approvals: BTreeMap<String, ApprovalView>,
    pub tool_outputs: BTreeMap<String, String>,
}

impl RunView {
    fn new(run_id: &str, session_id: &str) -> Self {
        Self {
            run_id: run_id.into(),
            session_id: session_id.into(),
            status: RunStatus::Unknown,
            last_sequence: 0,
            needs_replay: false,
            output: String::new(),
            reasoning: String::new(),
            transcript: Vec::new(),
            plan: None,
            todos: None,
            graph: None,
            tasks: BTreeMap::new(),
            evidence: Vec::new(),
            knowledge: Vec::new(),
            approvals: BTreeMap::new(),
            tool_outputs: BTreeMap::new(),
        }
    }

    fn push_transcript(&mut self, item: TranscriptItem) {
        self.transcript.push(item);
        if self.transcript.len() > MAX_TRANSCRIPT_ITEMS {
            let excess = self.transcript.len() - MAX_TRANSCRIPT_ITEMS;
            self.transcript.drain(..excess);
        }
    }

    fn append_bounded(value: &mut String, delta: &str, limit: usize) {
        value.push_str(delta);
        if value.len() > limit {
            let cut = value.len() - limit;
            let boundary = value
                .char_indices()
                .map(|(index, _)| index)
                .find(|index| *index >= cut)
                .unwrap_or(0);
            value.drain(..boundary);
        }
    }

    fn append_reasoning(&mut self, delta: &str) {
        Self::append_bounded(&mut self.reasoning, delta, MAX_REASONING_CHARS);
        self.push_transcript(TranscriptItem::AssistantReasoning(delta.into()));
    }

    fn append_tool_output(&mut self, call_id: &str, delta: &str) {
        let output = self.tool_outputs.entry(call_id.into()).or_default();
        Self::append_bounded(output, delta, MAX_TOOL_OUTPUT_CHARS);
        self.push_transcript(TranscriptItem::ToolOutput {
            call_id: call_id.into(),
            output: delta.into(),
        });
    }

    fn settle(&mut self, status: RunStatus) {
        self.status = status;
        self.approvals.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied { selected: bool },
    Duplicate { sequence: u64 },
    Gap { expected: u64, received: u64 },
    ProtocolMismatch { expected: u16, received: u16 },
}

#[derive(Debug, Default)]
pub struct AppState {
    pub connection: ConnectionState,
    pub protocol_version: Option<u16>,
    pub server_version: Option<String>,
    pub sessions: Vec<Session>,
    pub selected_session: Option<String>,
    pub active_run: Option<String>,
    pub runs: BTreeMap<String, RunView>,
    pub active_runs: Vec<ActiveRun>,
}

impl AppState {
    pub fn apply_event(&mut self, envelope: RigaEventEnvelope) -> ApplyOutcome {
        if envelope.protocol_version != PROTOCOL_VERSION {
            return ApplyOutcome::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                received: envelope.protocol_version,
            };
        }

        let run_id = envelope.run_id.clone();
        let selected = self.active_run.as_deref() == Some(run_id.as_str());
        let run = self
            .runs
            .entry(run_id.clone())
            .or_insert_with(|| RunView::new(&run_id, &envelope.session_id));

        if envelope.sequence <= run.last_sequence {
            return ApplyOutcome::Duplicate {
                sequence: envelope.sequence,
            };
        }
        let expected_sequence = run.last_sequence.saturating_add(1);
        let gap = run.last_sequence > 0 && envelope.sequence > expected_sequence;
        if gap {
            run.needs_replay = true;
        }
        run.last_sequence = envelope.sequence;

        match envelope.event {
            RigaEvent::RunStarted => run.status = RunStatus::Running,
            RigaEvent::TextDelta { delta } => {
                run.output.push_str(&delta);
                run.push_transcript(TranscriptItem::AssistantText(delta));
            }
            RigaEvent::ReasoningDelta { delta } => run.append_reasoning(&delta),
            RigaEvent::ToolCallStarted { call } => {
                run.push_transcript(TranscriptItem::ToolCall { call });
            }
            RigaEvent::ToolOutputDelta { call_id, delta } => {
                run.append_tool_output(&call_id, &delta);
            }
            RigaEvent::ToolResult { result } => {
                run.push_transcript(TranscriptItem::ToolResult { result });
            }
            RigaEvent::PlanUpdated { plan } => run.plan = Some(*plan),
            RigaEvent::TodoUpdated { list } => run.todos = Some(*list),
            RigaEvent::GraphUpdated { graph } => run.graph = Some(*graph),
            RigaEvent::TaskDependencyAdded { .. }
            | RigaEvent::TaskBlocked { .. }
            | RigaEvent::TaskRunnable { .. } => {}
            RigaEvent::EvidenceAdded { evidence } => {
                run.evidence.push(*evidence);
                trim_front(&mut run.evidence, MAX_EVIDENCE);
            }
            RigaEvent::EvidenceLinked { .. } => {}
            RigaEvent::KnowledgeCreated { knowledge } => {
                run.knowledge.push(*knowledge);
                trim_front(&mut run.knowledge, MAX_KNOWLEDGE);
            }
            RigaEvent::KnowledgeLinked { .. } => {}
            RigaEvent::TaskStarted { task } => {
                let id = task.id.clone();
                run.tasks.insert(
                    id,
                    TaskView {
                        state: task.state,
                        record: Some(*task),
                        elapsed_ms: 0,
                        result: None,
                    },
                );
            }
            RigaEvent::TaskStatus {
                task_id,
                state,
                elapsed_ms,
            } => {
                let task = run.tasks.entry(task_id).or_insert(TaskView {
                    record: None,
                    state,
                    elapsed_ms: 0,
                    result: None,
                });
                task.state = state;
                task.elapsed_ms = elapsed_ms;
                if state == TaskState::WaitingForApproval {
                    run.status = RunStatus::WaitingForApproval;
                }
            }
            RigaEvent::TaskCompleted {
                task_id,
                ok,
                result,
            } => {
                let task = run.tasks.entry(task_id.clone()).or_insert(TaskView {
                    record: None,
                    state: if ok {
                        TaskState::Completed
                    } else {
                        TaskState::Failed
                    },
                    elapsed_ms: 0,
                    result: None,
                });
                task.state = if ok {
                    TaskState::Completed
                } else {
                    TaskState::Failed
                };
                task.result = Some(result.clone());
                run.push_transcript(TranscriptItem::TaskResult {
                    task_id,
                    ok,
                    result,
                });
            }
            RigaEvent::ApprovalRequested {
                approval_id,
                task_id,
                tool,
                summary,
            } => {
                run.status = RunStatus::WaitingForApproval;
                run.approvals.insert(
                    approval_id.clone(),
                    ApprovalView {
                        approval_id: approval_id.clone(),
                        task_id,
                        tool: tool.clone(),
                        summary: summary.clone(),
                        resolved: None,
                    },
                );
                run.push_transcript(TranscriptItem::Approval {
                    approval_id,
                    tool,
                    summary,
                });
            }
            RigaEvent::ApprovalResolved {
                approval_id,
                approved,
                ..
            } => {
                if let Some(approval) = run.approvals.get_mut(&approval_id) {
                    approval.resolved = Some(approved);
                }
                run.approvals.remove(&approval_id);
                if run.status == RunStatus::WaitingForApproval {
                    run.status = RunStatus::Running;
                }
            }
            RigaEvent::RunCompleted { output } => {
                run.output = output.clone();
                run.push_transcript(TranscriptItem::System(format!("Run completed: {output}")));
                run.settle(RunStatus::Completed);
            }
            RigaEvent::RunFailed { message } => {
                run.push_transcript(TranscriptItem::System(format!("Run failed: {message}")));
                run.settle(RunStatus::Failed);
            }
        }

        if gap {
            ApplyOutcome::Gap {
                expected: expected_sequence,
                received: run.last_sequence,
            }
        } else {
            ApplyOutcome::Applied { selected }
        }
    }

    pub fn select_run(&mut self, run_id: Option<String>) {
        self.active_run = run_id;
    }

    pub fn run(&self, run_id: &str) -> Option<&RunView> {
        self.runs.get(run_id)
    }
}

fn trim_front<T>(values: &mut Vec<T>, max: usize) {
    if values.len() > max {
        values.drain(..values.len() - max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riga_kernel::events::RigaEvent;

    fn envelope(sequence: u64, event: RigaEvent) -> RigaEventEnvelope {
        RigaEventEnvelope {
            protocol_version: PROTOCOL_VERSION,
            event_id: format!("event-{sequence}"),
            session_id: "session-1".into(),
            run_id: "run-1".into(),
            sequence,
            timestamp: "now".into(),
            event,
        }
    }

    #[test]
    fn projects_streaming_text_reasoning_and_tools_without_flattening() {
        let mut app = AppState::default();
        app.select_run(Some("run-1".into()));
        app.apply_event(envelope(1, RigaEvent::RunStarted));
        app.apply_event(envelope(
            2,
            RigaEvent::ReasoningDelta {
                delta: "think".into(),
            },
        ));
        app.apply_event(envelope(
            3,
            RigaEvent::TextDelta {
                delta: "answer".into(),
            },
        ));
        app.apply_event(envelope(
            4,
            RigaEvent::ToolCallStarted {
                call: serde_json::json!({"name": "read"}),
            },
        ));
        app.apply_event(envelope(
            5,
            RigaEvent::ToolOutputDelta {
                call_id: "call-1".into(),
                delta: "file contents".into(),
            },
        ));

        let run = app.run("run-1").unwrap();
        assert_eq!(run.reasoning, "think");
        assert_eq!(run.output, "answer");
        assert_eq!(run.tool_outputs["call-1"], "file contents");
        assert!(matches!(
            run.transcript[0],
            TranscriptItem::AssistantReasoning(_)
        ));
        assert!(matches!(
            run.transcript[1],
            TranscriptItem::AssistantText(_)
        ));
        assert!(matches!(run.transcript[2], TranscriptItem::ToolCall { .. }));
        assert!(matches!(
            run.transcript[3],
            TranscriptItem::ToolOutput { .. }
        ));
    }

    #[test]
    fn duplicate_and_gap_sequences_are_explicit() {
        let mut app = AppState::default();
        assert!(matches!(
            app.apply_event(envelope(1, RigaEvent::RunStarted)),
            ApplyOutcome::Applied { .. }
        ));
        assert_eq!(
            app.apply_event(envelope(1, RigaEvent::RunStarted)),
            ApplyOutcome::Duplicate { sequence: 1 }
        );
        assert_eq!(
            app.apply_event(envelope(4, RigaEvent::TextDelta { delta: "x".into() })),
            ApplyOutcome::Gap {
                expected: 2,
                received: 4
            }
        );
        assert!(app.run("run-1").unwrap().needs_replay);
    }

    #[test]
    fn terminal_events_clear_approvals_and_remain_visible() {
        let mut app = AppState::default();
        app.apply_event(envelope(
            1,
            RigaEvent::ApprovalRequested {
                approval_id: "approval-1".into(),
                task_id: "task-1".into(),
                tool: "write".into(),
                summary: "change file".into(),
            },
        ));
        app.apply_event(envelope(
            2,
            RigaEvent::RunFailed {
                message: "denied".into(),
            },
        ));
        let run = app.run("run-1").unwrap();
        assert_eq!(run.status, RunStatus::Failed);
        assert!(run.approvals.is_empty());
        assert!(
            run.transcript.iter().any(
                |item| matches!(item, TranscriptItem::System(text) if text.contains("denied"))
            )
        );
    }

    #[test]
    fn rejects_protocol_mismatch_without_mutating_state() {
        let mut app = AppState::default();
        let mut event = envelope(1, RigaEvent::RunStarted);
        event.protocol_version += 1;
        assert_eq!(
            app.apply_event(event),
            ApplyOutcome::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                received: PROTOCOL_VERSION + 1
            }
        );
        assert!(app.runs.is_empty());
    }
}
