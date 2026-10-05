use riga_server::{ServerState, router};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("mcp-health-stdio") {
        run_health_stdio().await;
        return;
    }
    let address = std::env::var("RIGA_SERVER_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("bind RIGA server address");
    println!("RIGA server listening on {address}");
    axum::serve(listener, router(ServerState::default()))
        .await
        .expect("serve RIGA HTTP/SSE transport");
}

async fn run_health_stdio() {
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
                json!({ "protocolVersion": "2025-03-26", "serverInfo": { "name": "riga-health-stdio", "version": env!("CARGO_PKG_VERSION") } })
            }
            "tools/list" => {
                json!({ "tools": [{ "name": "health", "description": "Return RIGA agent kernel health", "inputSchema": { "type": "object", "properties": {} } }] })
            }
            "tools/call" => {
                json!({ "content": [{ "type": "text", "text": format!("RIGA kernel healthy · protocol {} · persistence {}", riga_kernel::PROTOCOL_VERSION, riga_server::secure_store::database_backend()) }] })
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
