use riga_kernel::{Agent, Health};
use riga_server::{
    ipc::{Attachment, IpcAttachment, IpcService, ProviderConfigured},
    ws::ProviderConfig,
    CreateSessionRequest, HealthResponse, ServerState,
};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Deserialize)]
struct LocalModelActionRequest {
    action: String,
    model_id: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApprovalRequest {
    run_id: String,
    approval_id: String,
    approved: bool,
    option: Option<String>,
}

#[tauri::command]
fn kernel_health(agent: State<'_, Agent>) -> Health {
    agent.health()
}

#[tauri::command]
async fn riga_transport_connect(
    app: AppHandle,
    service: State<'_, IpcService>,
) -> Result<(), String> {
    // Restore the saved provider so the settings form survives a restart, the
    // same way the WebSocket adapter receives it on `Hello`.
    if let Some(configured) = service.provider().await? {
        let _ = app.emit("riga://provider-configured", configured);
    }
    Ok(())
}

#[tauri::command]
fn riga_transport_disconnect() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
async fn riga_health(service: State<'_, IpcService>) -> Result<HealthResponse, String> {
    Ok(service.health())
}

#[tauri::command]
async fn riga_catalog(service: State<'_, IpcService>) -> Result<serde_json::Value, String> {
    Ok(serde_json::to_value(service.catalog().await).unwrap_or_default())
}

#[tauri::command]
async fn riga_list_sessions(
    service: State<'_, IpcService>,
) -> Result<Vec<riga_kernel::state::Session>, String> {
    Ok(service.list_sessions().await)
}

#[tauri::command]
async fn riga_create_session(
    service: State<'_, IpcService>,
    request: CreateSessionRequest,
) -> Result<riga_kernel::state::Session, String> {
    service.create_session(request).await
}

#[tauri::command]
async fn riga_list_mcp_registry(
    service: State<'_, IpcService>,
) -> Result<Vec<riga_server::catalog::McpServerSummary>, String> {
    Ok(service.list_mcp_registry().await)
}

#[tauri::command]
async fn riga_save_mcp_registry(
    service: State<'_, IpcService>,
    request: riga_server::McpRegistryRequest,
) -> Result<Vec<riga_server::catalog::McpServerSummary>, String> {
    service.save_mcp_registry(request).await
}

#[tauri::command]
async fn riga_configure_provider(
    app: AppHandle,
    service: State<'_, IpcService>,
    endpoint: String,
    api_key: String,
    model: String,
    reasoning_effort: String,
    kind: Option<riga_server::ws::ProviderKind>,
    api: Option<riga_server::ws::ProviderApi>,
    subagent_model: Option<String>,
) -> Result<ProviderConfigured, String> {
    let configured = service
        .configure_provider(ProviderConfig {
            endpoint,
            api_key,
            model,
            reasoning_effort,
            kind: kind.unwrap_or_default(),
            api: api.unwrap_or_default(),
            subagent_model,
        })
        .await?;
    let _ = app.emit("riga://provider-configured", configured.clone());
    Ok(configured)
}

async fn forward_events(
    app: AppHandle,
    mut events: tokio::sync::broadcast::Receiver<riga_kernel::events::RigaEventEnvelope>,
) {
    while let Ok(event) = events.recv().await {
        let terminal = matches!(
            event.event,
            riga_kernel::events::RigaEvent::RunCompleted { .. }
                | riga_kernel::events::RigaEvent::RunFailed { .. }
        );
        let _ = app.emit("riga://run-event", event);
        if terminal {
            break;
        }
    }
}

async fn forward_local_model_events(
    app: AppHandle,
    mut events: tokio::sync::broadcast::Receiver<riga_server::local_model::LocalModelEvent>,
) {
    while let Ok(event) = events.recv().await {
        if let Ok(payload) = serde_json::to_value(event) {
            let _ = app.emit("riga://local-model-event", payload);
        }
    }
}

#[tauri::command]
async fn riga_start_run(
    app: AppHandle,
    service: State<'_, IpcService>,
    run_id: String,
    session_id: String,
    prompt: String,
) -> Result<(), String> {
    let events = service.start_run(run_id, session_id, prompt).await?;
    tauri::async_runtime::spawn(forward_events(app, events));
    Ok(())
}

#[tauri::command]
async fn riga_resume_run(
    app: AppHandle,
    service: State<'_, IpcService>,
    run_id: String,
    after_sequence: u64,
) -> Result<(), String> {
    let events = service.subscribe_run(&run_id, after_sequence).await?;
    tauri::async_runtime::spawn(forward_events(app, events));
    Ok(())
}

#[tauri::command]
async fn riga_cancel_run(service: State<'_, IpcService>, run_id: String) -> Result<(), String> {
    if service.cancel_run(&run_id).await {
        Ok(())
    } else {
        Err("run not found".into())
    }
}

#[tauri::command]
async fn riga_respond_to_approval(
    service: State<'_, IpcService>,
    request: ApprovalRequest,
) -> Result<(), String> {
    if service
        .respond_to_approval(
            &request.approval_id,
            request.approved,
            request.option.as_deref() == Some("always"),
        )
        .await
    {
        Ok(())
    } else {
        Err(format!("approval not found for run {}", request.run_id))
    }
}

#[tauri::command]
async fn riga_local_models(
    service: State<'_, IpcService>,
) -> Result<riga_server::LocalModelOverview, String> {
    Ok(service.local_models().await)
}

#[tauri::command]
async fn riga_local_model_action(
    service: State<'_, IpcService>,
    request: LocalModelActionRequest,
) -> Result<(), String> {
    service
        .local_model_action(
            &request.action,
            request.model_id.as_deref(),
            request.path.as_deref(),
        )
        .await
}

#[tauri::command]
async fn riga_upload_attachment(
    service: State<'_, IpcService>,
    attachment: IpcAttachment,
) -> Result<Attachment, String> {
    service.upload_attachment(attachment).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let service = IpcService::new(ServerState::default());
    let local_model_events = service.subscribe_local_models();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Agent::new())
        .manage(service)
        .invoke_handler(tauri::generate_handler![
            kernel_health,
            riga_transport_connect,
            riga_transport_disconnect,
            riga_health,
            riga_catalog,
            riga_list_sessions,
            riga_create_session,
            riga_list_mcp_registry,
            riga_save_mcp_registry,
            riga_configure_provider,
            riga_start_run,
            riga_resume_run,
            riga_cancel_run,
            riga_respond_to_approval,
            riga_local_models,
            riga_local_model_action,
            riga_upload_attachment,
        ])
        .setup(move |app| {
            tauri::async_runtime::spawn(forward_local_model_events(
                app.handle().clone(),
                local_model_events,
            ));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
