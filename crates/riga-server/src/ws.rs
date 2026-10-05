use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use riga_kernel::events::{RigaEvent, RigaEventEnvelope};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    #[serde(default = "default_reasoning_effort")]
    pub reasoning_effort: String,
}

fn default_reasoning_effort() -> String {
    "low".into()
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

pub async fn upgrade(socket: WebSocket, workspace_root: PathBuf) {
    let (mut sender, mut receiver) = socket.split();
    let secure_store = crate::secure_store::SecureStore::from_env();
    let mut provider: Option<ProviderConfig> = secure_store
        .as_ref()
        .and_then(|store| store.load::<ProviderConfig>("provider").ok().flatten());
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
                    if let Some(config) = &provider
                        && send(
                            &mut sender,
                            ServerMessage::ProviderConfigured {
                                endpoint: config.endpoint.clone(),
                                model: config.model.clone(),
                                reasoning_effort: config.reasoning_effort.clone(),
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
                    if let Some(store) = &secure_store {
                        let _ = store.save("provider", &config);
                    }
                    provider = Some(config);
                    if send(
                        &mut sender,
                        ServerMessage::ProviderConfigured {
                            endpoint,
                            model,
                            reasoning_effort,
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
                Ok(ClientMessage::ToolCall {
                    call_id,
                    name,
                    input,
                }) => {
                    let result = execute_tool(&workspace_root, &name, input).await;
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
                    let Some(config) = provider.clone() else {
                        let _ = send(&mut sender, ServerMessage::Error { code: "provider_not_configured".into(), message: "Configure an OpenAI-compatible provider in Settings before starting a run.".into() }).await;
                        continue;
                    };
                    if send_provider_events(
                        &mut sender,
                        &config,
                        &workspace_root,
                        &run_id,
                        &session_id,
                        &prompt,
                    )
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

async fn execute_tool(
    workspace_root: &std::path::Path,
    name: &str,
    input: serde_json::Value,
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
            crate::catalog::execute_bash(
                workspace_root,
                input
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("bash requires command")?,
            )
            .await
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
        _ => Err(format!("unknown or unavailable tool: {name}")),
    }
}

struct ToolTrace {
    call: serde_json::Value,
    name: String,
    output: String,
    ok: bool,
}

struct AgentResult {
    output: String,
    traces: Vec<ToolTrace>,
}

async fn send_provider_events<S>(
    sender: &mut S,
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
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
    match call_openai_compatible(config, workspace_root, prompt).await {
        Ok(result) => {
            let mut sequence = 2;
            for trace in result.traces {
                let call_id = trace
                    .call
                    .get("call_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("tool-call")
                    .to_owned();
                send(
                    sender,
                    ServerMessage::Event {
                        envelope: envelope(
                            run_id,
                            session_id,
                            sequence,
                            RigaEvent::ToolCallStarted { call: trace.call },
                        ),
                    },
                )
                .await?;
                sequence += 1;
                send(sender, ServerMessage::Event { envelope: envelope(run_id, session_id, sequence, RigaEvent::ToolResult { result: serde_json::json!({"call_id": call_id, "name": trace.name, "output": trace.output, "ok": trace.ok}) }) }).await?;
                sequence += 1;
            }
            send(
                sender,
                ServerMessage::Event {
                    envelope: envelope(
                        run_id,
                        session_id,
                        sequence,
                        RigaEvent::TextDelta {
                            delta: result.output.clone(),
                        },
                    ),
                },
            )
            .await?;
            send(
                sender,
                ServerMessage::Event {
                    envelope: envelope(
                        run_id,
                        session_id,
                        sequence + 1,
                        RigaEvent::RunCompleted {
                            output: result.output,
                        },
                    ),
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

async fn call_openai_compatible(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
) -> Result<AgentResult, String> {
    if config.model.to_ascii_lowercase().starts_with("gpt-5") {
        return call_responses_api(config, workspace_root, prompt).await;
    }
    call_chat_with_tools(config, workspace_root, prompt).await
}

async fn call_chat_with_tools(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
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
    let mut messages = vec![serde_json::json!({ "role": "user", "content": prompt })];
    let mut traces = Vec::new();
    for _ in 0..24 {
        let mut request = client
            .post(&endpoint)
            .json(&completion_request_body_with_messages(
                &config.model,
                &messages,
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
                traces,
            });
        }
        messages.push(message);
        for tool_call in tool_calls {
            let call_id = tool_call
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool-call");
            let function = tool_call
                .get("function")
                .ok_or("provider returned malformed tool call")?;
            let name = function
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or("provider tool call has no name")?;
            let arguments = function
                .get("arguments")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("{}");
            let input: serde_json::Value = serde_json::from_str(arguments)
                .map_err(|e| format!("invalid arguments for {name}: {e}"))?;
            let result = execute_tool(workspace_root, name, input.clone()).await;
            let (ok, output) = match result {
                Ok(output) => (true, output),
                Err(error) => (false, format!("tool error: {error}")),
            };
            traces.push(ToolTrace {
                call: serde_json::json!({"call_id": call_id, "name": name, "arguments": input}),
                name: name.into(),
                output: output.clone(),
                ok,
            });
            messages.push(
                serde_json::json!({ "role": "tool", "tool_call_id": call_id, "content": output }),
            );
        }
    }
    Err("provider exceeded the maximum tool-call turns".into())
}

async fn call_responses_api(
    config: &ProviderConfig,
    workspace_root: &std::path::Path,
    prompt: &str,
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
    let mut input = serde_json::json!(prompt);
    let mut previous_response_id: Option<String> = None;
    let mut traces = Vec::new();
    for _ in 0..24 {
        let mut body = serde_json::json!({
            "model": config.model,
            "input": input,
            "max_output_tokens": 1024,
            "reasoning": { "effort": effort },
            "tools": responses_tool_schemas(),
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
                traces,
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
            let result = execute_tool(workspace_root, name, input_value.clone()).await;
            let (ok, output) = match result {
                Ok(output) => (true, output),
                Err(error) => (false, format!("tool error: {error}")),
            };
            let call_id = call
                .get("call_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool-call");
            traces.push(ToolTrace { call: serde_json::json!({"call_id": call_id, "name": name, "arguments": input_value}), name: name.into(), output: output.clone(), ok });
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

fn responses_tool_schemas() -> Vec<serde_json::Value> {
    tool_schemas().into_iter().filter_map(|tool| {
        let function = tool.get("function")?;
        Some(serde_json::json!({"type":"function", "name":function.get("name")?, "description":function.get("description")?, "parameters":function.get("parameters")?}))
    }).collect()
}

fn completion_request_body_with_messages(
    model: &str,
    messages: &[serde_json::Value],
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "max_completion_tokens": 2048,
        "tools": tool_schemas(),
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
            "Find workspace files by suffix pattern",
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
            "List, inspect, create, or update durable tasks; list or dispatch explore, plan, build, and review agents",
            serde_json::json!({"type":"object","properties":{"action":{"type":"string","enum":["list","inspect","create","update","agents","agent_list","dispatch","agent"]},"agent":{"type":"string","enum":["explore","plan","build","review","scout","planner","executor","worker","reviewer"]},"name":{"type":"string"},"prompt":{"type":"string"},"title":{"type":"string"},"description":{"type":"string"},"status":{"type":"string"},"task_id":{"type":"string"}},"required":[]}),
        ),
        function_schema(
            "skill",
            "Load a named repository skill document, or list all available skills when name is omitted",
            serde_json::json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
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
        let body = super::completion_request_body_with_messages(
            "gpt-5-nano",
            &[serde_json::json!({"role": "user", "content": "hello"})],
        );
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["model"], "gpt-5-nano");
    }

    #[test]
    fn provider_defaults_to_low_reasoning_effort_and_responses_tools_are_flat() {
        let config: ProviderConfig = serde_json::from_str(
            r#"{"endpoint":"https://api.example/v1","api_key":"key","model":"gpt-5-codex"}"#,
        )
        .unwrap();
        assert_eq!(config.reasoning_effort, "low");
        let tools = super::responses_tool_schemas();
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
}
