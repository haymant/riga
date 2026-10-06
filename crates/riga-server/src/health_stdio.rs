//! Stdio MCP health server.
//!
//! The MCP runtime launches this helper from `current_exe()` as
//! `«binary» mcp-health-stdio`. For the standalone `riga-server` binary that is
//! obvious, but any desktop shell that embeds the server shares the same
//! executable path — so the shell must answer this argument by serving stdio,
//! or it starts a second copy of its own GUI instead.

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Answer MCP JSON-RPC over stdin/stdout until EOF.
pub async fn serve() {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    while let Ok(Some(line)) = lines.next_line().await {
        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let method = request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let id = request
            .get("id")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let result = match method {
            "initialize" => {
                json!({ "protocolVersion": "2025-03-26", "capabilities": { "tools": {} }, "serverInfo": { "name": "riga-health-stdio", "version": env!("CARGO_PKG_VERSION") } })
            }
            "tools/list" => {
                json!({ "tools": [{ "name": "health", "description": "Return RIGA agent kernel health", "inputSchema": { "type": "object", "properties": {} } }] })
            }
            "tools/call" => {
                json!({ "content": [{ "type": "text", "text": format!("RIGA kernel healthy · protocol {} · persistence {}", riga_kernel::PROTOCOL_VERSION, crate::secure_store::database_backend()) }] })
            }
            _ => json!({ "error": { "code": -32601, "message": "method not found" } }),
        };
        let response =
            serde_json::to_string(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
                .unwrap();
        if stdout.write_all(response.as_bytes()).await.is_err()
            || stdout.write_all(b"\n").await.is_err()
        {
            break;
        }
        let _ = stdout.flush().await;
    }
}
