use riga_server::{ServerState, router};

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("mcp-health-stdio") {
        riga_server::health_stdio::serve().await;
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
