mod app;
mod input;
mod model;
mod terminal;
mod transport;
mod ui;

use clap::Parser;
use model::{CatalogEntry, ConnectionState, ProviderForm};
use riga_server::{CreateSessionRequest, ServerState};
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
    let transport = IpcTransport::new(ServerState::default());

    if args.tui {
        let mut sessions = transport.list_sessions().await;
        if sessions.is_empty() {
            sessions.push(
                transport
                    .create_session(CreateSessionRequest {
                        title: "Default chat".into(),
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
            local_models: Some(local_models),
            ..app::UiState::default()
        };
        if let Some(session_id) = app.state.selected_session.clone() {
            app.set_session_history(transport.session_history(&session_id).await);
        }
        app.set_catalog(parse_catalog(transport.catalog().await));
        let mut candidates = vec![
            "/help".into(),
            "/model".into(),
            "/settings".into(),
            "/skills".into(),
            "/mcp".into(),
        ];
        for entry in &app.catalog {
            candidates.push(format!("/{}/{}", entry.kind, entry.id));
            if entry.kind.contains("agent") || entry.kind.contains("skill") {
                candidates.push(format!("@{}", entry.id));
            }
        }
        if let Ok(entries) = std::fs::read_dir(riga_server::catalog::workspace_root()) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    candidates.push(format!("@{name}"));
                }
            }
        }
        candidates.sort();
        candidates.dedup();
        app.set_command_candidates(candidates);
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
            serde_json::to_string_pretty(&transport.list_sessions().await)
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
            let id = item
                .get("id")
                .or_else(|| item.get("name"))
                .or_else(|| item.get("path"))
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
