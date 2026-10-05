// Hide the console window on Windows release builds. Debug builds keep it so
// `tauri dev` output stays visible. The attribute only means anything on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use riga_kernel::{Agent, Health};
use tauri::State;

/// The one command this shell exposes: prove the embedded agent is alive.
///
/// `Agent` comes from the kernel, so the command is pure glue — it holds no
/// state and no logic of its own. Add a command by writing another function
/// that forwards to another `Agent` method.
#[tauri::command]
fn health(agent: State<'_, Agent>) -> Health {
    agent.health()
}

/// Start the desktop shell around one embedded RIGA agent.
///
/// This is the whole embedding: manage the kernel agent as shared state, expose
/// the commands the webview may call, then run.
///
/// A single file is enough for a desktop app. Tauri's own template splits this
/// `run()` into `lib.rs` so mobile targets can link it; add that back, plus
/// `[lib] crate-type = ["lib", "cdylib", "staticlib"]`, only if you need
/// iOS/Android.
fn main() {
    tauri::Builder::<tauri::Wry>::default()
        .manage(Agent::new())
        .invoke_handler(tauri::generate_handler![health])
        .run(tauri::generate_context!())
        .expect("error while running RIGA desktop application");
}
