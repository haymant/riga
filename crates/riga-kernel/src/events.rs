use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceNode {
    pub id: String,
    pub claim: String,
    pub source_ref: String,
    pub confidence: u8,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeNode {
    pub id: String,
    pub fact: String,
    pub source_run_id: String,
    pub confidence: u8,
}

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
    /// The planner published the current execution DAG.
    GraphUpdated {
        graph: Box<crate::task::Graph>,
    },
    TaskDependencyAdded {
        task_id: String,
        depends_on: String,
    },
    TaskBlocked {
        task_id: String,
        blocked_by: Vec<String>,
    },
    TaskRunnable {
        task_id: String,
    },
    EvidenceAdded {
        evidence: Box<EvidenceNode>,
    },
    EvidenceLinked {
        claim_id: String,
        evidence_id: String,
    },
    KnowledgeCreated {
        knowledge: Box<KnowledgeNode>,
    },
    KnowledgeLinked {
        knowledge_id: String,
        evidence_id: String,
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
    use super::{EvidenceNode, KnowledgeNode, RigaEvent, RigaEventEnvelope};
    use crate::task::{
        Graph, GraphNode, Plan, PlanStep, TaskRecord, TaskState, TodoItem, TodoList, TodoStatus,
    };

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

    #[test]
    fn evidence_and_knowledge_events_round_trip_over_the_wire() {
        let events = [
            RigaEvent::EvidenceAdded {
                evidence: Box::new(EvidenceNode {
                    id: "ev-1".into(),
                    claim: "symbol exists".into(),
                    source_ref: "src/lib.rs:12".into(),
                    confidence: 90,
                    task_id: Some("task-1".into()),
                }),
            },
            RigaEvent::EvidenceLinked {
                claim_id: "claim-1".into(),
                evidence_id: "ev-1".into(),
            },
            RigaEvent::KnowledgeCreated {
                knowledge: Box::new(KnowledgeNode {
                    id: "k-1".into(),
                    fact: "the kernel is transport-neutral".into(),
                    source_run_id: "run-1".into(),
                    confidence: 85,
                }),
            },
            RigaEvent::KnowledgeLinked {
                knowledge_id: "k-1".into(),
                evidence_id: "ev-1".into(),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<RigaEvent>(&json).unwrap(), event);
        }
    }

    #[test]
    fn graph_events_round_trip_over_the_wire() {
        let events = [
            RigaEvent::GraphUpdated {
                graph: Box::new(Graph {
                    title: "x".into(),
                    nodes: vec![GraphNode {
                        id: "runtime".into(),
                        profile: "explore".into(),
                        description: "inspect".into(),
                        prompt: "inspect".into(),
                        depends_on: vec![],
                    }],
                }),
            },
            RigaEvent::TaskDependencyAdded {
                task_id: "build".into(),
                depends_on: "runtime".into(),
            },
            RigaEvent::TaskRunnable {
                task_id: "runtime".into(),
            },
            RigaEvent::TaskBlocked {
                task_id: "build".into(),
                blocked_by: vec!["runtime".into()],
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<RigaEvent>(&json).unwrap(), event);
        }
    }
}
