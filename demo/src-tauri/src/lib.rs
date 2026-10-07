use riga_kernel::{Agent, Health};
use tauri::State;

#[tauri::command]
fn kernel_health(agent: State<'_, Agent>) -> Health {
    agent.health()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Agent::new())
        .invoke_handler(tauri::generate_handler![kernel_health])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
