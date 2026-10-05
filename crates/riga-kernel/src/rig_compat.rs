use futures_util::{Stream, StreamExt};
use rig_agent::agent::{MultiTurnStreamItem, StreamingError};
use rig_core::streaming::{Item, StreamEvent};

use crate::events::RigaEvent;

/// Convert Rig's stream items into the stable RIGA event vocabulary.
pub async fn collect_events<S>(mut stream: S) -> Vec<Result<RigaEvent, String>>
where
    S: Stream<Item = Result<MultiTurnStreamItem, StreamingError>> + Unpin,
{
    let mut events = vec![Ok(RigaEvent::RunStarted)];
    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(Item::Event(event))) => match event {
                StreamEvent::Text { text, .. } => {
                    events.push(Ok(RigaEvent::TextDelta { delta: text }));
                }
                StreamEvent::Reasoning { text, .. } => {
                    events.push(Ok(RigaEvent::ReasoningDelta { delta: text }));
                }
                StreamEvent::End { .. }
                | StreamEvent::Start { .. }
                | StreamEvent::Arguments { .. } => {}
            },
            Ok(MultiTurnStreamItem::ToolCall { tool_call }) => {
                events.push(Ok(RigaEvent::ToolCallStarted {
                    call: serde_json::to_value(tool_call).unwrap_or(serde_json::Value::Null),
                }));
            }
            Ok(MultiTurnStreamItem::ToolExecutionCommitted { tool_call }) => {
                events.push(Ok(RigaEvent::ToolResult {
                    result: serde_json::to_value(tool_call).unwrap_or(serde_json::Value::Null),
                }));
            }
            Ok(MultiTurnStreamItem::StreamUserItem(_)) => {}
            Ok(MultiTurnStreamItem::CompletionCall(_))
            | Ok(MultiTurnStreamItem::ModelTurnRetried { .. }) => {}
            Ok(MultiTurnStreamItem::FinalResponse(response)) => {
                events.push(Ok(RigaEvent::RunCompleted {
                    output: response.output,
                }));
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(Item::Unknown(_))) => {}
            Err(error) => events.push(Err(error.to_string())),
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::collect_events;
    use crate::events::RigaEvent;
    use rig_agent::test_utils::{
        MockAddTool, MockCompletionModel, MockFailingTool, MockStreamEvent, MockTurn,
    };
    use rig_core::tool::ToolErrorKind;

    #[tokio::test]
    async fn scripted_text_stream_delegates_to_rig_and_emits_ordered_riga_events() {
        let model = MockCompletionModel::from_stream_turns([[
            MockStreamEvent::text("hello "),
            MockStreamEvent::text("RIGA"),
            MockStreamEvent::final_response(Default::default()),
        ]]);
        let agent = rig_agent::AgentBuilder::new(model)
            .name("riga-phase-1")
            .preamble("Respond briefly.")
            .build();

        let events = collect_events(agent.prompt("Say hello").stream()).await;
        assert!(matches!(events.first(), Some(Ok(RigaEvent::RunStarted))));
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(RigaEvent::TextDelta { delta }) if delta == "hello "
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(RigaEvent::RunCompleted { output }) if output == "hello RIGA"
        )));
    }

    #[tokio::test]
    async fn scripted_tool_turn_uses_rigs_tool_loop() {
        let model = MockCompletionModel::from_stream_turns([
            [
                MockStreamEvent::tool_call("call-1", "add", serde_json::json!({"x": 2, "y": 3})),
                MockStreamEvent::final_response(Default::default()),
            ],
            [
                MockStreamEvent::text("done"),
                MockStreamEvent::final_response(Default::default()),
            ],
        ]);
        let agent = rig_agent::AgentBuilder::new(model)
            .name("riga-tool-proof")
            .tool(MockAddTool)
            .build();

        let events = collect_events(agent.prompt("Add 2 and 3").max_turns(2).stream()).await;
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(RigaEvent::ToolCallStarted { call }) if call.to_string().contains("add")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(RigaEvent::ToolResult { result }) if result.to_string().contains("add")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(RigaEvent::RunCompleted { output }) if output == "done"
        )));
    }

    #[test]
    fn pinned_rig_version_is_documented_by_the_dependency_lock() {
        let lock = include_str!("../../../Cargo.lock");
        assert!(lock.contains("name = \"rig-agent\""));
        assert!(lock.contains("name = \"rig-core\""));
    }

    #[tokio::test]
    async fn scripted_tool_failure_is_handled_by_rigs_loop() {
        let model = MockCompletionModel::from_turns([
            MockTurn::tool_call("call-fail", "flaky_tool", serde_json::json!({})),
            MockTurn::text("recovered"),
        ]);
        let agent = rig_agent::AgentBuilder::new(model)
            .tool(MockFailingTool::new(ToolErrorKind::Other))
            .build();

        let response = agent
            .prompt("Use the flaky tool")
            .max_turns(2)
            .await
            .expect("Rig should feed a tool failure back into the scripted model");
        assert_eq!(response.output, "recovered");
    }

    #[tokio::test]
    async fn invalid_tool_call_and_max_turns_are_typed_rig_failures() {
        let invalid_model = MockCompletionModel::from_turns([MockTurn::tool_call(
            "call-invalid",
            "missing_tool",
            serde_json::json!({}),
        )]);
        let invalid_agent = rig_agent::AgentBuilder::new(invalid_model).build();
        assert!(invalid_agent.prompt("Call a missing tool").await.is_err());

        let max_turn_model = MockCompletionModel::text("never called");
        let max_turn_agent = rig_agent::AgentBuilder::new(max_turn_model).build();
        assert!(
            max_turn_agent
                .prompt("No turns")
                .max_turns(0)
                .await
                .is_err()
        );
    }
}
