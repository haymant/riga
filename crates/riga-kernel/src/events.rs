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
