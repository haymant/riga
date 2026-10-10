mod model;
mod transport;

use clap::Parser;
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
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();
    let transport = IpcTransport::new(ServerState::default());

    // TUI-0 intentionally has a deterministic headless surface. Interactive
    // Ratatui mode is added in the next phase, while IPC remains the default
    // runtime boundary for both modes.
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
