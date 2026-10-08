use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RigaEvent {
    RunStarted,
    TextDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    ToolCallStarted {
        call: serde_json::Value,
    },
    ToolOutputDelta {
        call_id: String,
        delta: String,
    },
    ToolResult {
        result: serde_json::Value,
    },
    /// The orchestrator rewrote its plan. Carries the whole plan so a client
    /// that attached mid-run can render the current state without replaying.
    PlanUpdated {
        plan: Box<crate::task::Plan>,
    },
    /// The agent's working list changed.
    TodoUpdated {
        list: Box<crate::task::TodoList>,
    },
    /// A subagent task was dispatched.
    TaskStarted {
        task: Box<crate::task::TaskRecord>,
    },
    /// A subagent's state or progress label changed.
    TaskStatus {
        task_id: String,
        state: crate::task::TaskState,
        elapsed_ms: u64,
    },
    /// A subagent settled, with its result summary.
    TaskCompleted {
        task_id: String,
        ok: bool,
        result: String,
    },
    /// A tool call is blocked on the user. The task stays alive until resolved.
    ApprovalRequested {
        approval_id: String,
        task_id: String,
        tool: String,
        summary: String,
    },
    /// The user's decision on an approval.
    ApprovalResolved {
        approval_id: String,
        approved: bool,
        reason: Option<String>,
    },
    RunCompleted {
        output: String,
    },
    RunFailed {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RigaEventEnvelope {
    pub protocol_version: u16,
    pub event_id: String,
    pub session_id: String,
    pub run_id: String,
    pub sequence: u64,
    pub timestamp: String,
    pub event: RigaEvent,
}

#[cfg(test)]
mod tests {
    use super::{RigaEvent, RigaEventEnvelope};
    use crate::task::{Plan, PlanStep, TaskRecord, TaskState, TodoItem, TodoList, TodoStatus};

    #[test]
    fn task_and_plan_events_round_trip_over_the_wire() {
        let envelope = RigaEventEnvelope {
            protocol_version: 1,
            event_id: "run-1-3".into(),
            session_id: "s".into(),
            run_id: "run-1".into(),
            sequence: 3,
            timestamp: "now".into(),
            event: RigaEvent::TaskStarted {
                task: Box::new(TaskRecord {
                    id: "task-2".into(),
                    parent_id: Some("task-1".into()),
                    agent: "explore".into(),
                    description: "find the entry point".into(),
                    model: "haiku".into(),
                    state: TaskState::Running,
                    started_at: "now".into(),
                    result: None,
                }),
            },
        };
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains("\"TaskStarted\""), "{json}");
        assert!(json.contains("\"agent\":\"explore\""), "{json}");
        let decoded: RigaEventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn plan_and_todo_events_serialize_their_full_state() {
        let plan = RigaEvent::PlanUpdated {
            plan: Box::new(Plan {
                title: "Build a service".into(),
                steps: vec![PlanStep {
                    id: "probe".into(),
                    label: "add a probe endpoint".into(),
                    description: None,
                }],
                active_index: 0,
            }),
        };
        assert!(serde_json::to_string(&plan).unwrap().contains("probe"));

        let todos = RigaEvent::TodoUpdated {
            list: Box::new(TodoList {
                title: None,
                revision: 1,
                items: vec![TodoItem {
                    id: "1".into(),
                    text: "inspect".into(),
                    description: None,
                    status: TodoStatus::Done,
                    reason: None,
                }],
            }),
        };
        assert!(serde_json::to_string(&todos).unwrap().contains("\"done\""));
    }

    #[test]
    fn task_lifecycle_events_round_trip_all_decision_fields() {
        let events = vec![
            RigaEvent::TaskStatus {
                task_id: "task-2".into(),
                state: TaskState::WaitingForApproval,
                elapsed_ms: 42,
            },
            RigaEvent::TaskCompleted {
                task_id: "task-2".into(),
                ok: false,
                result: "approval denied".into(),
            },
            RigaEvent::ApprovalRequested {
                approval_id: "approval-1".into(),
                task_id: "task-2".into(),
                tool: "write".into(),
                summary: "modify a file".into(),
            },
            RigaEvent::ApprovalResolved {
                approval_id: "approval-1".into(),
                approved: false,
                reason: Some("user declined".into()),
            },
        ];

        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let decoded: RigaEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, event, "event failed to round-trip: {json}");
        }
    }
}
