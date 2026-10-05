use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RigaEvent {
    RunStarted,
    TextDelta { delta: String },
    ReasoningDelta { delta: String },
    ToolCallStarted { call: serde_json::Value },
    ToolResult { result: serde_json::Value },
    RunCompleted { output: String },
    RunFailed { message: String },
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
