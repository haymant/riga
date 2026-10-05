use riga_desktop::{HealthResponse, RigaState, build_state};
use tauri::State;

#[tauri::command]
fn health_command(state: State<'_, RigaState>) -> Result<HealthResponse, String> {
    Ok(HealthResponse {
        protocol_version: state.kernel_version.protocol_version,
    })
}

fn main() {
    tauri::Builder::<tauri::Wry>::default()
        .manage(build_state())
        .invoke_handler(tauri::generate_handler![health_command])
        .run(tauri::generate_context!())
        .expect("error while running RIGA desktop application");
}
