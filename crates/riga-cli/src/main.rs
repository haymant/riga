mod app;
mod input;
mod markdown;
mod model;
mod terminal;
mod transport;
mod ui;

use clap::Parser;
use model::{CatalogEntry, CompletionItem, ConnectionState, ProviderForm};
use riga_server::{CreateSessionRequest, ServerState, catalog::McpServerSummary};
use serde_json::json;
use transport::IpcTransport;

#[derive(Debug, Parser)]
#[command(name = "riga-cli", version, about = "RIGA IPC client and terminal UI")]
struct Args {
    /// Use stable machine-readable output instead of the future interactive TUI.
    #[arg(long)]
    plain: bool,
    /// Print the server health response.
    #[arg(long)]
    health: bool,
    /// Print all persisted sessions.
    #[arg(long)]
    list_sessions: bool,
    /// Create a session with this title.
    #[arg(long)]
    create_session: Option<String>,
    /// Workspace used with --create-session.
    #[arg(long, default_value = ".")]
    workspace: String,
    /// Print the discovered catalog.
    #[arg(long)]
    catalog: bool,
    /// Start the interactive Ratatui terminal UI.
    #[arg(long)]
    tui: bool,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();
    let transport = IpcTransport::new(ServerState::load().await?);

    if args.tui {
        let mut sessions = transport.list_sessions().await?;
        if sessions.is_empty() {
            sessions.push(
                transport
                    .create_session(CreateSessionRequest {
                        title: "New session".into(),
                        workspace: ".".into(),
                    })
                    .await?,
            );
        }
        let active_runs = transport.list_active_runs().await;
        let local_models = transport.local_models().await;
        let mut app = app::UiState {
            state: model::AppState {
                connection: ConnectionState::Connected,
                selected_session: sessions.first().map(|session| session.id.clone()),
                sessions,
                active_runs,
                ..model::AppState::default()
            },
            follow_output: true,
            reasoning_collapsed: true,
            tools_collapsed: true,
            local_models: Some(local_models),
            ..app::UiState::default()
        };
        if let Some(session_id) = app.state.selected_session.clone() {
            app.set_session_history(transport.session_history(&session_id).await);
        }
        let catalog_data = transport.catalog().await;
        app.set_catalog(parse_catalog(catalog_data.clone()));
        let mcp_registry = transport.mcp_registry().await;
        app.set_completion_candidates(completion_candidates(&catalog_data, &mcp_registry));
        if let Ok(Some(provider)) = transport.provider().await {
            app.set_provider(ProviderForm {
                endpoint: provider.endpoint,
                model: provider.model,
                reasoning_effort: provider.reasoning_effort,
                kind: provider.kind,
                api: provider.api,
                subagent_model: provider.subagent_model.unwrap_or_default(),
                ..ProviderForm::default()
            });
        }
        return app::run_with_transport(app, transport).await;
    }

    // The headless flags remain deterministic; interactive mode uses the same
    // IPC runtime boundary and is enabled explicitly with --tui.
    if args.health || (!args.list_sessions && args.create_session.is_none() && !args.catalog) {
        println!(
            "{}",
            serde_json::to_string_pretty(&transport.health()).map_err(|error| error.to_string())?
        );
    }

    if args.list_sessions {
        println!(
            "{}",
            serde_json::to_string_pretty(&transport.list_sessions().await?)
                .map_err(|error| error.to_string())?
        );
    }

    if let Some(title) = args.create_session {
        let session = transport
            .create_session(CreateSessionRequest {
                title,
                workspace: args.workspace,
            })
            .await?;
        println!(
            "{}",
            serde_json::to_string_pretty(&session).map_err(|error| error.to_string())?
        );
    }

    if args.catalog {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!(transport.catalog().await))
                .map_err(|error| error.to_string())?
        );
    }

    let _ = args.plain;
    Ok(())
}

fn parse_catalog(
    catalog: std::collections::BTreeMap<String, serde_json::Value>,
) -> Vec<CatalogEntry> {
    let mut entries = Vec::new();
    for (kind, value) in catalog {
        let Some(items) = value.as_array() else {
            continue;
        };
        for item in items {
            let id_value = if kind == "files" {
                item.get("path")
                    .or_else(|| item.get("id"))
                    .or_else(|| item.get("name"))
            } else {
                item.get("id")
                    .or_else(|| item.get("name"))
                    .or_else(|| item.get("path"))
            };
            let id = id_value
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            entries.push(CatalogEntry {
                id: id.into(),
                kind: kind.clone(),
                description: item
                    .get("description")
                    .or_else(|| item.get("purpose"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into(),
                insert_text: item
                    .get("insert_text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into(),
                requires_approval: item
                    .get("requires_approval")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            });
        }
    }
    entries
}

fn completion_candidates(
    catalog: &std::collections::BTreeMap<String, serde_json::Value>,
    mcp_servers: &[McpServerSummary],
) -> Vec<CompletionItem> {
    let mut candidates = Vec::new();

    for (command, detail) in [
        ("/model", "Choose or set the active remote model"),
        ("/models", "Alias for /model"),
        ("/tools", "Browse built-in tools"),
        ("/mcp", "Browse configured MCP servers and tools"),
        ("/skills", "Browse discovered skills"),
        ("/settings", "Open provider settings"),
        ("/local-models", "Open the local model manager"),
        ("/new", "Start a new session"),
        ("/clear", "Alias for /new; keep the old session in history"),
        ("/resume", "Open session history"),
        ("/sessions", "Alias for /resume"),
        ("/rename", "Rename the current session"),
        ("/status", "Show connection, model, and session status"),
        ("/help", "Open the Info panel"),
        ("/exit", "Exit RIGA-CLI"),
        ("/quit", "Alias for /exit"),
    ] {
        candidates.push(completion('/', "Commands", command, detail));
    }

    if let Some(skills) = catalog.get("skills").and_then(serde_json::Value::as_array) {
        for skill in skills {
            let name = candidate_id(skill);
            if name.is_empty() {
                continue;
            }
            let label = format!("/skills/{name}");
            let insertion = format!("Use the {name} skill: ");
            candidates.push(completion_with_insert(
                '/',
                "Skills",
                &label,
                candidate_detail(skill),
                &insertion,
            ));
        }
    }

    if let Some(tools) = catalog.get("tools").and_then(serde_json::Value::as_array) {
        for tool in tools {
            let name = candidate_id(tool);
            if name.is_empty() {
                continue;
            }
            let label = format!("/tools/{name}");
            let mut detail = candidate_detail(tool).to_owned();
            if tool
                .get("requires_approval")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                if !detail.is_empty() {
                    detail.push_str(" · ");
                }
                detail.push_str("approval required");
            }
            let insertion = tool
                .get("insert_text")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Use the {name} tool: "));
            candidates.push(completion_with_insert(
                '/', "Tools", &label, &detail, &insertion,
            ));
        }
    }

    if let Some(agents) = catalog.get("agents").and_then(serde_json::Value::as_array) {
        for agent in agents {
            let name = candidate_id(agent);
            if !name.is_empty() {
                let label = format!("@{name}");
                candidates.push(completion(
                    '@',
                    "Subagents",
                    &label,
                    candidate_detail(agent),
                ));
            }
        }
    }

    if let Some(files) = catalog.get("files").and_then(serde_json::Value::as_array) {
        for file in files {
            let path = file
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if !path.is_empty() {
                let label = format!("@{path}");
                candidates.push(completion('@', "Files", &label, "Workspace file"));
            }
        }
    }

    let mut server_tools = std::collections::BTreeMap::<String, Vec<String>>::new();
    if let Some(servers) = catalog
        .get("mcp_servers")
        .and_then(serde_json::Value::as_array)
    {
        for server in servers {
            let name = server
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let tools = server
                .get("tools")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned);
            server_tools.entry(name.into()).or_default().extend(tools);
        }
    }
    for server in mcp_servers {
        server_tools
            .entry(server.name.clone())
            .or_default()
            .extend(server.tools.iter().cloned());
    }

    for (name, mut tools) in server_tools {
        tools.sort();
        tools.dedup();
        let server_label = format!("/mcp/{name}");
        candidates.push(completion(
            '/',
            "MCP servers",
            &server_label,
            "Configured MCP server",
        ));
        for tool in &tools {
            let label = format!("/mcp/{name}/{tool}");
            let insertion = format!("Use the MCP tool {name}/{tool}: ");
            candidates.push(completion_with_insert(
                '/',
                "MCP server-tools",
                &label,
                &format!("Tool exposed by {name}"),
                &insertion,
            ));
        }
    }

    candidates.sort_by(|left, right| {
        left.trigger
            .cmp(&right.trigger)
            .then_with(|| left.category.cmp(&right.category))
            .then_with(|| left.label.cmp(&right.label))
    });
    candidates.dedup_by(|left, right| left.trigger == right.trigger && left.label == right.label);
    candidates
}

fn completion(trigger: char, category: &str, label: &str, detail: &str) -> CompletionItem {
    completion_with_insert(trigger, category, label, detail, label)
}

fn completion_with_insert(
    trigger: char,
    category: &str,
    label: &str,
    detail: &str,
    insert_text: &str,
) -> CompletionItem {
    CompletionItem {
        trigger,
        category: category.into(),
        label: label.into(),
        detail: detail.into(),
        insert_text: insert_text.into(),
    }
}

fn candidate_id(item: &serde_json::Value) -> &str {
    item.get("id")
        .or_else(|| item.get("name"))
        .or_else(|| item.get("path"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

fn candidate_detail(item: &serde_json::Value) -> &str {
    item.get("description")
        .or_else(|| item.get("purpose"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_completions_separate_slash_tools_from_file_and_agent_mentions() {
        let catalog = std::collections::BTreeMap::from([
            (
                "tools".into(),
                json!([{
                    "id": "read",
                    "description": "Read a workspace file",
                    "insert_text": "Use the read tool: "
                }]),
            ),
            (
                "skills".into(),
                json!([{
                    "name": "rust-review",
                    "description": "Review Rust changes"
                }]),
            ),
            (
                "agents".into(),
                json!([{"name": "researcher", "description": "Research agent"}]),
            ),
            (
                "files".into(),
                json!([{"name": "main.rs", "path": "src/main.rs"}]),
            ),
            (
                "mcp_servers".into(),
                json!([{"name": "riga-health-stdio", "tools": ["health"]}]),
            ),
        ]);
        let mcp = vec![McpServerSummary {
            name: "docs".into(),
            command: "http-stream".into(),
            args: Vec::new(),
            tools: vec!["search".into()],
            transport: Some("http".into()),
            url: None,
            api_key_configured: false,
        }];
        let items = completion_candidates(&catalog, &mcp);
        let labels = items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"/tools/read"));
        assert!(labels.contains(&"/skills/rust-review"));
        assert!(labels.contains(&"/mcp/docs/search"));
        assert!(labels.contains(&"/mcp/riga-health-stdio/health"));
        assert!(labels.contains(&"/model"));
        assert!(labels.contains(&"/tools"));
        assert!(labels.contains(&"/mcp"));
        assert!(labels.contains(&"/skills"));
        assert!(labels.contains(&"/settings"));
        assert!(labels.contains(&"/local-models"));
        assert!(labels.contains(&"/new"));
        assert!(labels.contains(&"/resume"));
        assert!(labels.contains(&"/status"));
        assert!(labels.contains(&"/quit"));
        assert!(labels.contains(&"@src/main.rs"));
        assert!(labels.contains(&"@researcher"));
        assert!(!labels.contains(&"@rust-review"));
        assert_eq!(
            items
                .iter()
                .find(|item| item.label == "/tools/read")
                .unwrap()
                .insert_text,
            "Use the read tool: "
        );
    }
}
