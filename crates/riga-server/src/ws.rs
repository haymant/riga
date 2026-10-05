use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use riga_kernel::events::{RigaEvent, RigaEventEnvelope};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        client_version: String,
    },
    StartRun {
        run_id: String,
        session_id: String,
        prompt: String,
    },
    CancelRun {
        run_id: String,
    },
    Approval {
        run_id: String,
        approval_id: String,
        approved: bool,
    },
    Ping {
        nonce: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ready {
        protocol_version: u16,
        server_version: &'static str,
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
}

pub async fn upgrade(socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::Hello { .. }) => {
                    let _ = send(
                        &mut sender,
                        ServerMessage::Ready {
                            protocol_version: riga_kernel::PROTOCOL_VERSION,
                            server_version: "0.1.0",
                        },
                    )
                    .await;
                }
                Ok(ClientMessage::Ping { nonce }) => {
                    let _ = send(&mut sender, ServerMessage::Pong { nonce }).await;
                }
                Ok(ClientMessage::Approval {
                    run_id,
                    approval_id,
                    approved,
                }) => {
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
                Ok(ClientMessage::StartRun {
                    run_id,
                    session_id,
                    prompt,
                }) => {
                    let events = kernel_demo_events(&run_id, &session_id, &prompt);
                    for envelope in events {
                        if send(&mut sender, ServerMessage::Event { envelope })
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
                            code: "invalid_message".into(),
                            message: error.to_string(),
                        },
                    )
                    .await;
                }
            },
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

async fn send<S>(sender: &mut S, message: ServerMessage) -> Result<(), S::Error>
where
    S: SinkExt<Message> + Unpin,
{
    let payload = serde_json::to_string(&message).expect("WebSocket message is serializable");
    sender.send(Message::Text(payload.into())).await
}

fn kernel_demo_events(run_id: &str, session_id: &str, prompt: &str) -> Vec<RigaEventEnvelope> {
    let output = format!("Kernel accepted: {}", prompt.trim());
    vec![
        envelope(run_id, session_id, 1, RigaEvent::RunStarted),
        envelope(
            run_id,
            session_id,
            2,
            RigaEvent::TextDelta {
                delta: "I’m connected to the RIGA kernel over WebSocket. ".into(),
            },
        ),
        envelope(
            run_id,
            session_id,
            3,
            RigaEvent::TextDelta {
                delta: output.clone(),
            },
        ),
        envelope(run_id, session_id, 4, RigaEvent::RunCompleted { output }),
    ]
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

#[cfg(test)]
mod tests {
    use super::{ClientMessage, ServerMessage, kernel_demo_events};
    use riga_kernel::events::RigaEvent;

    #[test]
    fn protocol_accepts_start_and_serializes_kernel_events() {
        let message: ClientMessage = serde_json::from_str(
            r#"{"type":"start_run","run_id":"run-1","session_id":"session-1","prompt":"hello"}"#,
        )
        .unwrap();
        assert!(matches!(message, ClientMessage::StartRun { .. }));
        let events = kernel_demo_events("run-1", "session-1", "hello");
        assert_eq!(events.len(), 4);
        assert!(matches!(events[1].event, RigaEvent::TextDelta { .. }));
        let ready = serde_json::to_string(&ServerMessage::Ready {
            protocol_version: 1,
            server_version: "0.1.0",
        })
        .unwrap();
        assert!(ready.contains("ready"));
    }

    #[test]
    fn protocol_rejects_unknown_message_types() {
        let result = serde_json::from_str::<ClientMessage>(
            r#"{"type":"execute_shell","command":"rm -rf /"}"#,
        );
        assert!(result.is_err());
    }
}
