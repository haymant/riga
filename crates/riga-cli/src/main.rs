mod app;
mod input;
mod markdown;
mod model;
mod report;
mod template;
mod terminal;
mod transport;
mod ui;

use clap::Parser;
use model::{CatalogEntry, CompletionItem, ConnectionState, ProviderForm};
use riga_server::{CreateSessionRequest, ServerState, catalog::McpServerSummary, ipc::McpToolStatus};
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
    /// Print the connected MCP servers and their tools (markdown).
    #[arg(long)]
    tools: bool,
    /// Call an MCP tool: `<server>/<tool>` (e.g. `riga-flow/price_put`).
    #[arg(long)]
    call: Option<String>,
    /// JSON input for --call: inline JSON, or `@path/to/file.json`.
    #[arg(long)]
    input: Option<String>,
    /// PriceRequest template for --call. Defaults to the embedded worst-of
    /// Phoenix baseline (`templates/termsheet1.request.json`).
    #[arg(long)]
    template: Option<String>,
    /// Patch the template: `path=value`, repeatable. e.g. `--set trade.strike=0.6`
    /// or `--set market.vols[0]=0.48`.
    #[arg(long = "set", value_name = "PATH=VALUE")]
    set: Vec<String>,
    /// Emit JSON instead of markdown for the flags that support it (--tools, --call).
    #[arg(long)]
    json: bool,
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
        let initial_session = sessions
            .iter()
            .find(|session| !session.archived)
            .or_else(|| sessions.first())
            .cloned();
        let history_archived_view = initial_session
            .as_ref()
            .is_some_and(|session| session.archived);
        let active_runs = transport.list_active_runs().await;
        let local_models = transport.local_models().await;
        let mut app = app::UiState {
            state: model::AppState {
                connection: ConnectionState::Connected,
                selected_session: initial_session.as_ref().map(|session| session.id.clone()),
                sessions,
                active_runs,
                ..model::AppState::default()
            },
            follow_output: true,
            reasoning_collapsed: true,
            tools_collapsed: true,
            local_models: Some(local_models),
            history_archived_view,
            ..app::UiState::default()
        };
        if let Some(session_id) = app.state.selected_session.clone() {
            app.set_session_history(transport.session_history(&session_id).await);
        }
        // Preflight: connect the registered MCP servers so their tools are
        // advertised, and warn about any that are unreachable (stdout is the TUI,
        // so the warning goes to stderr).
        for server in transport.mcp_status().await {
            if !server.connected {
                eprintln!(
                    "riga-cli: MCP server '{}' is unreachable — its tools will be missing (build/start it, then restart riga-cli)",
                    server.name
                );
            }
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
    if args.health
        || (!args.list_sessions
            && args.create_session.is_none()
            && !args.catalog
            && !args.tools
            && args.call.is_none())
    {
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

    if args.tools {
        let status = transport.mcp_status().await;
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&status).map_err(|error| error.to_string())?
            );
        } else {
            print!("{}", render_mcp_tools(&status));
        }
    }

    if let Some(target) = args.call.as_deref() {
        let (server, tool) = target
            .split_once('/')
            .ok_or_else(|| format!("--call expects <server>/<tool>, got '{target}'"))?;
        let base_input: serde_json::Value = match args.input.as_deref() {
            None => serde_json::json!({}),
            Some(text) if text.starts_with('@') => {
                let path = &text[1..];
                let body = std::fs::read_to_string(path)
                    .map_err(|error| format!("{path}: {error}"))?;
                serde_json::from_str(&body).map_err(|error| format!("{path}: {error}"))?
            }
            Some(text) => {
                serde_json::from_str(text).map_err(|error| format!("--input: {error}"))?
            }
        };
        // The template is used when one is given explicitly, or implicitly when
        // there are overrides to apply to the shipped baseline.
        let mut request: Option<serde_json::Value> = if let Some(path) = args.template.as_deref() {
            let body = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
            Some(
                serde_json::from_str(&body).map_err(|error| format!("{path}: {error}"))?,
            )
        } else if !args.set.is_empty() {
            Some(template::default_template())
        } else if tool == "price_trade" {
            // `price_trade` requires a request; without one the call would be a
            // guaranteed validation error. Default to the shipped template (the
            // canonical termsheet1 request), so the baseline matches `price_put`.
            Some(template::default_template())
        } else {
            None
        };
        if let Some(request) = request.as_mut() {
            template::apply_overrides(request, &args.set)?;
        }
        let input = template::compose_input(tool, Some(base_input), request);
        let output = transport.call_mcp_tool(server, tool, input).await?;
        if args.json {
            println!("{output}");
        } else {
            print!("{}", report::tool_output_to_markdown(&output));
        }
    }

    let _ = args.plain;
    Ok(())
}

/// Markdown listing of the registered MCP servers, their connection state and
/// their tools — the headless counterpart of the TUI `/mcp` browser.
fn render_mcp_tools(status: &[McpToolStatus]) -> String {
    let mut out = format!("# MCP servers ({})\n", status.len());
    for server in status {
        let state = if server.connected { "connected" } else { "unreachable" };
        out.push_str(&format!(
            "\n## {} — {} · {} tools\n\n",
            server.name,
            state,
            server.tools.len()
        ));
        if server.tools.is_empty() {
            out.push_str(if server.connected {
                "_connected but advertises no tools_\n"
            } else {
                "_not connected — build/start the server, then retry_\n"
            });
            continue;
        }
        let mut tools = server.tools.clone();
        tools.sort();
        let rendered = tools
            .iter()
            .map(|tool| format!("`{tool}`"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("{rendered}\n"));
    }
    out
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
        ("/export", "Export the current conversation as JSONL"),
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
        assert!(labels.contains(&"/export"));
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

    #[test]
    fn mcp_tools_markdown_lists_servers_and_flags_unreachable() {
        let status = vec![
            McpToolStatus {
                name: "riga-flow".into(),
                connected: true,
                tools: vec!["b_tool".into(), "a_tool".into()],
            },
            McpToolStatus {
                name: "tac-engine".into(),
                connected: false,
                tools: Vec::new(),
            },
        ];
        let text = render_mcp_tools(&status);
        assert!(text.contains("# MCP servers (2)"), "{text}");
        assert!(text.contains("## riga-flow — connected · 2 tools"), "{text}");
        // Tools are sorted, so the listing is stable.
        assert!(text.contains("`a_tool`, `b_tool`"), "{text}");
        assert!(text.contains("## tac-engine — unreachable · 0 tools"), "{text}");
        assert!(text.contains("not connected"), "{text}");
    }

    #[test]
    fn mcp_tools_markdown_flags_a_connected_server_without_tools() {
        let status = vec![McpToolStatus {
            name: "empty".into(),
            connected: true,
            tools: Vec::new(),
        }];
        let text = render_mcp_tools(&status);
        assert!(text.contains("advertises no tools"), "{text}");
    }
}
