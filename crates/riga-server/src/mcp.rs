use std::{collections::HashMap, sync::Arc, time::Duration};

use rig_agent::tool::{ToolContext, server::ToolServer, server::ToolServerHandle};
use rig_rmcp::{McpClientHandler, rmcp};
use rmcp::{
    model::ClientInfo,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::sync::RwLock;

use crate::catalog::McpServerRecord;

#[derive(Clone)]
pub struct McpRuntime {
    tools: ToolServerHandle,
    connected: Arc<RwLock<Vec<String>>>,
    aliases: Arc<RwLock<HashMap<String, String>>>,
}

impl McpRuntime {
    pub fn new() -> Self {
        Self {
            tools: ToolServer::new().owner("riga-mcp").run(),
            connected: Arc::new(RwLock::new(Vec::new())),
            aliases: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn connect_records(&self, records: &[McpServerRecord]) {
        for record in records.iter().cloned() {
            self.connect_record(record).await;
        }
    }

    async fn connect_record(&self, record: McpServerRecord) {
        if self
            .connected_names()
            .await
            .iter()
            .any(|name| name == &record.summary.name)
        {
            return;
        }
        let handler = McpClientHandler::new(ClientInfo::default(), self.tools.clone())
            .with_refresh_timeout(Duration::from_secs(20));
        let name = record.summary.name.clone();
        let connected = self.connected.clone();
        let service = match record.summary.transport.as_deref() {
            Some("stdio") | None => {
                let command_name = if record.summary.name == "riga-health-stdio" {
                    std::env::current_exe()
                        .map_err(|error| error.to_string())
                        .unwrap_or_else(|_| record.summary.command.clone().into())
                } else {
                    record.summary.command.clone().into()
                };
                let mut command = tokio::process::Command::new(command_name);
                if record.summary.name == "riga-health-stdio" {
                    command.arg("mcp-health-stdio");
                } else {
                    command.args(&record.summary.args);
                }
                let transport = match TokioChildProcess::new(command.configure(|cmd| {
                    cmd.env("RIGA_MCP_SERVER", &name);
                })) {
                    Ok(transport) => transport,
                    Err(error) => {
                        tracing::warn!(server = %name, %error, "MCP stdio process failed to start");
                        return;
                    }
                };
                handler.connect(transport).await
            }
            Some("http") | Some("streamable-http") => {
                let Some(url) = record.summary.url.clone() else {
                    tracing::warn!(server = %name, "MCP HTTP record has no URL");
                    return;
                };
                let mut config = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url);
                if let Some(key) = record.api_key.filter(|value| !value.trim().is_empty()) {
                    config = config.auth_header(key);
                }
                let transport = rmcp::transport::StreamableHttpClientTransport::from_config(config);
                handler.connect(transport).await
            }
            Some(transport) => {
                tracing::warn!(server = %name, transport, "unsupported MCP transport");
                return;
            }
        };
        let waiting = match service {
            Ok(service) => service.waiting(),
            Err(error) => {
                tracing::warn!(server = %name, %error, "MCP connection failed");
                return;
            }
        };
        {
            let mut aliases = self.aliases.write().await;
            for tool in &record.summary.tools {
                aliases.insert(
                    qualified_tool_name(&record.summary.name, tool),
                    tool.clone(),
                );
                if record.summary.name == "riga-health-stdio" {
                    aliases.insert(qualified_tool_name("rig-health-stdio", tool), tool.clone());
                }
            }
        }
        connected.write().await.push(name.clone());
        tokio::spawn(async move {
            match waiting.await {
                Ok(reason) => {
                    tracing::info!(server = %name, ?reason, "MCP connection ended")
                }
                Err(error) => {
                    tracing::warn!(server = %name, %error, "MCP connection task failed")
                }
            }
            connected.write().await.retain(|item| item != &name);
        });
    }

    pub async fn tool_definitions(&self) -> Vec<rig_core::completion::ToolDefinition> {
        let definitions = self.tools.tool_defs(None).await.unwrap_or_default();
        let aliases = self.aliases.read().await;
        let mut result = definitions.clone();
        for (alias, target) in aliases.iter() {
            if let Some(definition) = definitions.iter().find(|item| &item.name == target) {
                let mut alias_definition = definition.clone();
                alias_definition.name = alias.clone();
                alias_definition.description = format!(
                    "MCP server tool alias for {target}; use this when the user explicitly requests the corresponding MCP server"
                );
                result.push(alias_definition);
            }
        }
        result
    }

    pub async fn execute(&self, name: &str, input: &serde_json::Value) -> Result<String, String> {
        let actual_name = self
            .aliases
            .read()
            .await
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned());
        let mut context = ToolContext::new();
        let result = self
            .tools
            .execute(
                &actual_name,
                &serde_json::to_string(input).map_err(|error| error.to_string())?,
                &mut context,
            )
            .await;
        if let Some(error) = result.error() {
            return Err(error.to_string());
        }
        Ok(result.output().render())
    }

    pub async fn connected_names(&self) -> Vec<String> {
        self.connected.read().await.clone()
    }
}

impl Default for McpRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn qualified_tool_name(server: &str, tool: &str) -> String {
    format!(
        "mcp_{}_{}",
        sanitize_tool_segment(server),
        sanitize_tool_segment(tool)
    )
}

fn sanitize_tool_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

pub fn definition_to_openai(
    definition: &rig_core::completion::ToolDefinition,
) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": definition.name,
            "description": definition.description,
            "parameters": definition.parameters,
        }
    })
}

pub fn definition_to_responses(
    definition: &rig_core::completion::ToolDefinition,
) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "name": definition.name,
        "description": definition.description,
        "parameters": definition.parameters,
    })
}

pub fn merge_tool_schemas(
    builtin: Vec<serde_json::Value>,
    mcp: Vec<rig_core::completion::ToolDefinition>,
) -> Vec<serde_json::Value> {
    let mut result = builtin;
    let mut names = result
        .iter()
        .filter_map(|tool| {
            tool.pointer("/function/name")
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_owned)
        .collect::<std::collections::HashSet<_>>();
    for definition in mcp {
        if names.insert(definition.name.clone()) {
            result.push(definition_to_openai(&definition));
        }
    }
    result
}

pub fn merge_response_tool_schemas(
    builtin: Vec<serde_json::Value>,
    mcp: Vec<rig_core::completion::ToolDefinition>,
) -> Vec<serde_json::Value> {
    let mut result = builtin
        .into_iter()
        .filter_map(|tool| {
            let function = tool.get("function")?;
            Some(serde_json::json!({
                "type": "function",
                "name": function.get("name")?,
                "description": function.get("description")?,
                "parameters": function.get("parameters")?,
            }))
        })
        .collect::<Vec<_>>();
    let mut names = result
        .iter()
        .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .collect::<std::collections::HashSet<_>>();
    for definition in mcp {
        if names.insert(definition.name.clone()) {
            result.push(definition_to_responses(&definition));
        }
    }
    result
}
