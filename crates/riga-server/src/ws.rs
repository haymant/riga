use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use riga_kernel::events::{RigaEvent, RigaEventEnvelope};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
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
    ProviderConfigured {
        model: String,
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

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    content: serde_json::Value,
}

pub async fn upgrade(socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let mut provider: Option<ProviderConfig> = None;
    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => match serde_json::from_str::<ClientMessage>(&text) {
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
                }
                Ok(ClientMessage::ConfigureProvider(config)) => {
                    let model = config.model.clone();
                    provider = Some(config);
                    if send(&mut sender, ServerMessage::ProviderConfigured { model })
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
                    let Some(config) = provider.clone() else {
                        let _ = send(&mut sender, ServerMessage::Error { code: "provider_not_configured".into(), message: "Configure an OpenAI-compatible provider in Settings before starting a run.".into() }).await;
                        continue;
                    };
                    if send_provider_events(&mut sender, &config, &run_id, &session_id, &prompt)
                        .await
                        .is_err()
                    {
                        return;
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

async fn send_provider_events<S>(
    sender: &mut S,
    config: &ProviderConfig,
    run_id: &str,
    session_id: &str,
    prompt: &str,
) -> Result<(), S::Error>
where
    S: SinkExt<Message> + Unpin,
{
    send(
        sender,
        ServerMessage::Event {
            envelope: envelope(run_id, session_id, 1, RigaEvent::RunStarted),
        },
    )
    .await?;
    match call_openai_compatible(config, prompt).await {
        Ok(output) => {
            send(
                sender,
                ServerMessage::Event {
                    envelope: envelope(
                        run_id,
                        session_id,
                        2,
                        RigaEvent::TextDelta {
                            delta: output.clone(),
                        },
                    ),
                },
            )
            .await?;
            send(
                sender,
                ServerMessage::Event {
                    envelope: envelope(run_id, session_id, 3, RigaEvent::RunCompleted { output }),
                },
            )
            .await
        }
        Err(error) => {
            send(
                sender,
                ServerMessage::Event {
                    envelope: envelope(
                        run_id,
                        session_id,
                        2,
                        RigaEvent::RunFailed { message: error },
                    ),
                },
            )
            .await
        }
    }
}

async fn call_openai_compatible(config: &ProviderConfig, prompt: &str) -> Result<String, String> {
    if config.model.to_ascii_lowercase().starts_with("gpt-5") {
        return call_responses_api(config, prompt).await;
    }
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
    let mut request = client
        .post(endpoint)
        .json(&completion_request_body(&config.model, prompt));
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
        let hint = if status == reqwest::StatusCode::FORBIDDEN {
            " Check that this API key is authorized for the selected model and that the provider account permits inference."
        } else if status == reqwest::StatusCode::UNAUTHORIZED {
            " Check that the API key is valid and has not expired."
        } else {
            ""
        };
        return Err(format!(
            "provider returned HTTP {status}:{hint} {}",
            redact_body(&body),
        ));
    }
    let completion: ChatCompletionResponse =
        serde_json::from_str(&body).map_err(|e| format!("invalid provider response: {e}"))?;
    completion
        .choices
        .first()
        .and_then(|choice| content_text(&choice.message.content))
        .ok_or_else(|| "provider returned no choices".into())
}

async fn call_responses_api(config: &ProviderConfig, prompt: &str) -> Result<String, String> {
    let endpoint = if config
        .endpoint
        .trim_end_matches('/')
        .ends_with("/responses")
    {
        config.endpoint.trim_end_matches('/').to_string()
    } else {
        format!("{}/responses", config.endpoint.trim_end_matches('/'))
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let mut request = client.post(endpoint).json(&serde_json::json!({
        "model": config.model,
        "input": prompt,
        "max_output_tokens": 1024,
        "reasoning": { "effort": "minimal" },
    }));
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
            "provider Responses API returned HTTP {status}: {}",
            redact_body(&body)
        ));
    }
    let response: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("invalid provider Responses API response: {e}"))?;
    let output = response
        .get("output")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("message"))
        .filter_map(|item| item.get("content").and_then(serde_json::Value::as_array))
        .flatten()
        .filter_map(|part| {
            if part.get("type").and_then(serde_json::Value::as_str) == Some("output_text") {
                part.get("text").and_then(serde_json::Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    if output.is_empty() {
        return Err(format!(
            "provider Responses API returned no output text (status: {})",
            response
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
        ));
    }
    Ok(output)
}

fn completion_request_body(model: &str, prompt: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": prompt }],
        "stream": false,
        "max_completion_tokens": 2048,
    })
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

#[cfg(test)]
mod tests {
    use super::{ClientMessage, ProviderConfig, ServerMessage};
    use riga_kernel::events::RigaEvent;

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
    fn completion_request_uses_strict_max_completion_tokens() {
        let body = super::completion_request_body("gpt-5-nano", "hello");
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["model"], "gpt-5-nano");
    }
}
