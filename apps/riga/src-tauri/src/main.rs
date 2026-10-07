// Hide the console window on Windows release builds. Debug builds keep it so
// `tauri dev` output stays visible. The attribute only means anything on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(debug_assertions))]
use std::{sync::mpsc, thread};

use riga_kernel::{Agent, Health};
use tauri::{State, WebviewUrl, WebviewWindowBuilder, webview::NewWindowResponse};

/// Loopback origin of the server this process hosts, or an empty string when an
/// external kernel is expected (development). Kept as managed state so the
/// webview can ask for it once at startup instead of guessing the port.
struct ServerUrl(String);

/// The one kernel command this shell exposes: prove the embedded agent is alive.
///
/// The agent owns the behaviour, so this is pure glue. Add a command by writing
/// another function that forwards to another `Agent` method.
#[tauri::command]
fn health(agent: State<'_, Agent>) -> Health {
    agent.health()
}

/// Origin the React surface must resolve every request against.
///
/// Empty string means "same origin as the page", which is how `tauri:dev` and
/// the browser build reach the kernel through the Vite proxy. A real origin is
/// returned by the packaged build, where this process hosts `riga-server` on an
/// OS-assigned loopback port.
#[tauri::command]
fn server_url(server_url: State<'_, ServerUrl>) -> String {
    server_url.0.clone()
}

/// Host `riga-server` inside the desktop process and return its origin.
///
/// This is the whole reason the packaged app works: without a host, the webview
/// loads from `tauri://localhost`, where a relative `/ws` becomes
/// `ws://localhost/ws` and every `/local-models` call hits the asset protocol
/// instead of the kernel. Binding an OS-assigned port (`127.0.0.1:0`) keeps two
/// installs from colliding; `RIGA_SERVER_ADDRESS` overrides it for debugging.
#[cfg(not(debug_assertions))]
fn start_server() -> String {
    let bind_address =
        std::env::var("RIGA_SERVER_ADDRESS").unwrap_or_else(|_| "127.0.0.1:0".to_string());
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("riga-server".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build RIGA server runtime");
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::bind(&bind_address)
                    .await
                    .expect("bind RIGA server listener");
                let address = listener
                    .local_addr()
                    .expect("read RIGA server listener address");
                ready_sender
                    .send(format!("http://{address}"))
                    .expect("report RIGA server listener address");
                axum::serve(
                    listener,
                    riga_server::router(riga_server::ServerState::default()),
                )
                .await
                .expect("serve RIGA server");
            });
        })
        .expect("spawn RIGA server thread");
    ready_receiver
        .recv()
        .expect("receive RIGA server listener address")
}

/// Development keeps one kernel: the `beforeDevCommand` process that Vite
/// proxies to. Starting a second server here would race for the port and make
/// `npm run dev` non-deterministic.
#[cfg(debug_assertions)]
fn start_server() -> String {
    String::new()
}

fn main() {
    // The embedded `riga-server` launches its stdio MCP helper from the current
    // executable, so this same binary is invoked as `riga-desktop
    // mcp-health-stdio`. Answer that here, before Tauri starts: otherwise each
    // helper boots a second copy of the GUI, which embeds its own server and
    // spawns more helpers — an escalating storm of windows.
    if std::env::args().nth(1).as_deref() == Some("mcp-health-stdio") {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build MCP health runtime")
            .block_on(riga_server::health_stdio::serve());
        return;
    }
    // Must run before the Tauri/GTK runtime starts; a no-op off Linux.
    riga_shell::prepare_linux_display();
    let runtime_server_url = start_server();
    tauri::Builder::<tauri::Wry>::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Agent::new())
        .manage(ServerUrl(runtime_server_url))
        .invoke_handler(tauri::generate_handler![health, server_url])
        .setup(|app| {
            build_main_window(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running RIGA desktop application");
}

/// Create the one application window.
///
/// Built here rather than declared in `tauri.conf.json` so it can carry an
/// `on_new_window` handler. Tauri's default is to open a *second* window for
/// `window.open` and for every `target="_blank"` link, including relative or
/// non-web URLs the opener plugin does not redirect — which reads as the app
/// spawning blank copies of itself. Deny every such request and send web links
/// to the user's browser.
fn build_main_window(app: &mut tauri::App) -> tauri::Result<()> {
    let handle = app.handle().clone();
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("RIGA")
        .inner_size(1280.0, 800.0)
        .resizable(true)
        .on_new_window(move |url, _features| {
            use tauri_plugin_opener::OpenerExt;
            if matches!(url.scheme(), "http" | "https" | "mailto" | "tel") {
                let _ = handle.opener().open_url(url.as_str(), None::<&str>);
            }
            NewWindowResponse::Deny
        })
        .build()?;
    Ok(())
}
