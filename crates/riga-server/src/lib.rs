#![doc = "RIGA HTTP/SSE transport adapter."]

use std::path::PathBuf;

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_stream::stream;
use axum::{
    Json, Router,
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use riga_kernel::{
    events::{RigaEvent, RigaEventEnvelope},
    state::Session,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

pub mod catalog;
pub mod health_stdio;
pub mod ipc;
pub mod local_model;
pub mod mcp;
pub mod secure_store;
pub mod workspace;
pub mod ws;

pub const ADAPTER_NAME: &str = "riga-server";

#[derive(Clone)]
pub struct ServerState {
    pub(crate) sessions: Arc<RwLock<Vec<Session>>>,
    pub(crate) next_id: Arc<AtomicU64>,
    pub(crate) workspace_root: PathBuf,
    pub(crate) secure_store: Option<Arc<secure_store::SecureStore>>,
    pub(crate) mcp_registry: Arc<RwLock<Vec<catalog::McpServerRecord>>>,
    pub(crate) mcp_runtime: mcp::McpRuntime,
    /// Per-session conversation history, keyed by session id. Fed back to the
    /// model on the next turn so a follow-up has the context it refers to.
    pub(crate) transcripts:
        Arc<RwLock<std::collections::HashMap<String, Vec<ws::ConversationTurn>>>>,
    /// Local GGUF download/inference runtime. Cheap to construct: the llama.cpp
    /// backend is only initialized on the first model load, and the process-wide
    /// singleton is shared, so this stays inert until a model is actually used.
    pub(crate) local_models: Arc<local_model::LocalModelRuntime>,
    /// Pending tool approvals, shared across sockets so a run keeps waiting on an
    /// approval even if the socket that requested it reconnects.
    pub(crate) approvals: ws::ApprovalBroker,
    /// Runs currently executing, so any socket can follow, cancel, or resume one
    /// and a run survives the socket that started it.
    pub(crate) runs: ws::RunRegistry,
}

impl Default for ServerState {
    fn default() -> Self {
        let secure_store = secure_store::SecureStore::from_env().map(Arc::new);
        let sessions = secure_store
            .as_ref()
            .and_then(|store| store.load::<Vec<Session>>("sessions").ok().flatten())
            .unwrap_or_default();
        let workspace_root = catalog::workspace_root();
        let transcripts = secure_store
            .as_ref()
            .and_then(|store| {
                store
                    .load::<std::collections::HashMap<String, Vec<ws::ConversationTurn>>>(
                        "transcripts",
                    )
                    .ok()
                    .flatten()
            })
            .unwrap_or_default();
        let mcp_registry = secure_store
            .as_ref()
            .and_then(|store| {
                store
                    .load::<Vec<catalog::McpServerRecord>>("mcp_registry")
                    .ok()
                    .flatten()
            })
            .or_else(|| {
                std::fs::read_to_string(workspace_root.join(".riga-mcp-registry.json"))
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok())
            })
            .unwrap_or_default();
        Self {
            sessions: Arc::new(RwLock::new(sessions)),
            next_id: Arc::new(AtomicU64::new(1)),
            workspace_root,
            secure_store,
            mcp_registry: Arc::new(RwLock::new(mcp_registry)),
            mcp_runtime: mcp::McpRuntime::new(),
            transcripts: Arc::new(RwLock::new(transcripts)),
            local_models: Arc::new(local_model::LocalModelRuntime::default()),
            approvals: ws::ApprovalBroker::default(),
            runs: ws::RunRegistry::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub title: String,
    pub workspace: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub protocol_version: u16,
    pub adapter: &'static str,
    pub persistence_backend: &'static str,
}

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/catalog", get(tool_catalog))
        .route(
            "/mcp/registry",
            get(list_mcp_registry).post(save_mcp_registry),
        )
        .route("/mcp/health", post(mcp_health_http))
        .route("/attachments", post(upload_attachment))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/runs/{run_id}/events", get(stream_events))
        .route("/ws", get(ws_upgrade))
        .route(
            "/local-models",
            get(list_local_models).post(local_model_action),
        )
        .route("/local-models/catalog", get(local_model_catalog))
        .route("/local-models/downloads", get(local_model_downloads))
        .route(
            "/local-models/downloads/events",
            get(local_model_download_events),
        )
        .layer(cors_layer())
        .with_state(state)
}

/// The packaged desktop webview loads from `tauri://localhost`, so every call it
/// makes to the loopback server is cross-origin. Loopback binds hold no cookies
/// and are reachable only from this machine, so a permissive policy is safe and
/// lets the same surface run unchanged in a browser (same-origin, no CORS) and
/// in the desktop shell (cross-origin). Without it the browser build hides the
/// problem behind the Vite proxy, and the packaged build fails on `fetch` and
/// `EventSource` with an opaque network error.
fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}

async fn ws_upgrade(
    State(state): State<ServerState>,
    upgrade: axum::extract::ws::WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| {
        let runtime = state.mcp_runtime.clone();
        let registry = state.mcp_registry.clone();
        async move {
            let mut records = registry.read().await.clone();
            records.push(catalog::McpServerRecord {
                summary: catalog::builtin_health_stdio(),
                api_key: None,
            });
            records.push(catalog::McpServerRecord {
                summary: catalog::builtin_health_http(),
                api_key: None,
            });
            runtime.connect_records(&records).await;
            ws::upgrade(
                socket,
                state.workspace_root.clone(),
                runtime,
                state.local_models.clone(),
                state.transcripts.clone(),
                state.secure_store.clone(),
                state.approvals.clone(),
                state.runs.clone(),
            )
            .await;
        }
    })
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        protocol_version: riga_kernel::PROTOCOL_VERSION,
        adapter: ADAPTER_NAME,
        persistence_backend: secure_store::database_backend(),
    })
}

// ---------------------------------------------------------------------------
// Local GGUF model manager
// ---------------------------------------------------------------------------

/// Everything the model-picker UI needs to render in one round trip: the closed
/// curated list, what is on disk, what is loaded, and which accelerator this
/// build can actually use. Reporting the accelerator matters because a binary
/// built without `cuda` silently falls back to the CPU, and a user waiting on a
/// slow first token deserves to know that before they blame the model.
async fn list_local_models(State(state): State<ServerState>) -> Json<LocalModelOverview> {
    Json(LocalModelOverview {
        accelerator: local_model::accelerator_label(),
        catalog: state.local_models.curated_catalog(),
        installed: state.local_models.list_installed().unwrap_or_default(),
        loaded: state.local_models.loaded_file_name(),
    })
}

async fn local_model_catalog(State(state): State<ServerState>) -> Json<serde_json::Value> {
    Json(serde_json::json!(state.local_models.curated_catalog()))
}

/// Polled by the picker to render download progress. A poll rather than a push
/// stream keeps the browser build free of an EventSource dependency, and
/// download progress is coarse-grained enough that polling costs nothing.
async fn local_model_downloads(State(state): State<ServerState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "models_dir": state.local_models.models_dir_public(),
        "accelerator": local_model::accelerator_label(),
    }))
}

/// Server-sent events for download progress and terminal download state.
/// Subscribing before POSTing `/local-models` avoids missing the first event.
async fn local_model_download_events(
    State(state): State<ServerState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut receiver = state.local_models.subscribe();
    let stream = async_stream::stream! {
        // Comment frames defeat proxy/browser idle buffering, which otherwise
        // holds the first progress event until enough bytes accumulate.
        yield Ok(Event::default().comment("stream-open"));
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let payload = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
                    yield Ok(Event::default().event("local-model").data(payload));
                }
                // A lagging reader dropped events; the UI re-syncs from the
                // installed list, so this is not fatal.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    yield Ok(Event::default().comment("lagged"));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalModelOverview {
    pub accelerator: &'static str,
    pub catalog: Vec<local_model::CuratedModelEntry>,
    pub installed: Vec<local_model::InstalledModel>,
    pub loaded: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LocalModelRequest {
    /// One of `download`, `cancel`, `load`, `unload`.
    pub action: String,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

async fn local_model_action(
    State(state): State<ServerState>,
    Json(request): Json<LocalModelRequest>,
) -> impl IntoResponse {
    let bad = |message: &str| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response()
    };
    match request.action.as_str() {
        "download" => {
            let Some(model_id) = request
                .model_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
            else {
                return bad("model_id is required to download a model");
            };
            let model_id = model_id.to_owned();
            let runtime = state.local_models.clone();
            // Reject unknown ids and duplicates here rather than in the spawned
            // task: the task's error is only logged, so a bad id would answer
            // 202 "started" and the picker would wait on a bar that never moves.
            if let Some(blocker) = runtime.download_blocker(&model_id) {
                return bad(&blocker);
            }
            let for_task = model_id.clone();
            // The transfer outlives the request, so it runs detached and reports
            // through the event stream rather than holding the connection open
            // for the length of a multi-gigabyte download.
            tokio::spawn(async move {
                if let Err(error) = runtime.start_download(&for_task).await {
                    tracing::error!(model_id = %for_task, %error, "local model download failed");
                }
            });
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "started": model_id })),
            )
                .into_response()
        }
        "cancel" => {
            let Some(model_id) = request
                .model_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
            else {
                return bad("model_id is required to cancel a download");
            };
            match state.local_models.cancel_download(model_id) {
                Ok(()) => StatusCode::ACCEPTED.into_response(),
                Err(error) => bad(&error),
            }
        }
        "load" => {
            let Some(path) = request
                .path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty())
            else {
                return bad("path is required to load a model");
            };
            match state.local_models.load_model(path).await {
                Ok(()) => {
                    Json(serde_json::json!({ "loaded": state.local_models.loaded_file_name() }))
                        .into_response()
                }
                Err(error) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({ "error": error })),
                )
                    .into_response(),
            }
        }
        "unload" => match state.local_models.unload_model() {
            Ok(()) => Json(serde_json::json!({ "loaded": Option::<String>::None })).into_response(),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error })),
            )
                .into_response(),
        },
        other => bad(&format!("unknown local model action: {other}")),
    }
}

async fn tool_catalog(
    State(state): State<ServerState>,
) -> Json<std::collections::BTreeMap<String, serde_json::Value>> {
    let mut result = catalog::catalog(&state.workspace_root);
    let mut mcp_servers = vec![
        catalog::builtin_health_stdio(),
        catalog::builtin_health_http(),
    ];
    mcp_servers.extend(
        state
            .mcp_registry
            .read()
            .await
            .iter()
            .map(|record| record.summary.clone()),
    );
    result.insert(
        "mcp_servers".into(),
        serde_json::to_value(mcp_servers).unwrap(),
    );
    Json(result)
}

#[derive(Debug, Deserialize)]
pub struct McpRegistryRequest {
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
    pub api_key: Option<String>,
}

async fn list_mcp_registry(
    State(state): State<ServerState>,
) -> Json<Vec<catalog::McpServerSummary>> {
    Json(
        state
            .mcp_registry
            .read()
            .await
            .iter()
            .map(|record| record.summary.clone())
            .collect(),
    )
}

async fn save_mcp_registry(
    State(state): State<ServerState>,
    Json(request): Json<McpRegistryRequest>,
) -> impl IntoResponse {
    let name = request.name.trim();
    if name.is_empty() || !matches!(request.transport.as_str(), "stdio" | "http") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "name and transport (stdio or http) are required" })),
        )
            .into_response();
    }
    if request.transport == "http" && request.url.as_deref().unwrap_or("").trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "HTTP stream URL is required" })),
        )
            .into_response();
    }
    let summary = catalog::McpServerSummary {
        name: name.into(),
        command: request
            .command
            .clone()
            .unwrap_or_else(|| "http-stream".into()),
        args: request.args.clone(),
        tools: if name.starts_with("riga-health-") {
            vec!["health".into()]
        } else {
            Vec::new()
        },
        transport: Some(request.transport.clone()),
        url: request.url.clone(),
        api_key_configured: request
            .api_key
            .as_deref()
            .is_some_and(|key| !key.is_empty()),
    };
    let record = catalog::McpServerRecord {
        summary: summary.clone(),
        api_key: request.api_key.filter(|key| !key.is_empty()),
    };
    let mut registry = state.mcp_registry.write().await;
    registry.retain(|existing| existing.summary.name != summary.name);
    registry.push(record);
    let records = registry.clone();
    drop(registry);
    let persist_result = if let Some(store) = &state.secure_store {
        store.save("mcp_registry", &records)
    } else {
        std::fs::write(
            state.workspace_root.join(".riga-mcp-registry.json"),
            serde_json::to_vec_pretty(&records).unwrap_or_default(),
        )
        .map_err(|error| error.to_string())
    };
    if let Err(error) = persist_result {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response();
    }
    (StatusCode::CREATED, Json(summary)).into_response()
}

#[derive(Debug, Deserialize)]
struct McpJsonRpcRequest {
    method: String,
    #[serde(default)]
    id: serde_json::Value,
}

async fn mcp_health_http(
    State(_state): State<ServerState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<McpJsonRpcRequest>,
) -> impl IntoResponse {
    if let Some(expected) = std::env::var("RIGA_MCP_HEALTH_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
        && headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(&format!("Bearer {expected}"))
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid MCP API key" })),
        )
            .into_response();
    }
    let result = match request.method.as_str() {
        "tools/list" => {
            serde_json::json!({ "tools": [{ "name": "health", "description": "Return RIGA agent kernel health", "inputSchema": { "type": "object", "properties": {} } }] })
        }
        "tools/call" => {
            serde_json::json!({ "content": [{ "type": "text", "text": format!("RIGA kernel healthy · protocol {} · persistence {}", riga_kernel::PROTOCOL_VERSION, secure_store::database_backend()) }] })
        }
        "initialize" => {
            serde_json::json!({ "protocolVersion": "2025-03-26", "capabilities": { "tools": {} }, "serverInfo": { "name": ADAPTER_NAME, "version": env!("CARGO_PKG_VERSION") } })
        }
        _ => serde_json::json!({ "error": { "code": -32601, "message": "method not found" } }),
    };
    Json(serde_json::json!({ "jsonrpc": "2.0", "id": request.id, "result": result }))
        .into_response()
}

#[derive(Debug, Deserialize)]
struct AttachmentQuery {
    /// Session whose worktree receives the file. Empty falls back to the base
    /// workspace for a client that predates session-scoped uploads.
    session: Option<String>,
}

async fn upload_attachment(
    State(state): State<ServerState>,
    Query(query): Query<AttachmentQuery>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let Some(field) = (multipart.next_field().await).ok().flatten() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "a file field is required" })),
        )
            .into_response();
    };
    let original_name = field.file_name().unwrap_or("attachment").to_owned();
    let bytes = match field.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("unable to read attachment: {error}") })),
            )
                .into_response();
        }
    };
    // Store inside the session's worktree so the agent's tools find the file at
    // the relative path the prompt advertises.
    let session = query.session.unwrap_or_default();
    let dir = match crate::workspace::ensure_attachment_dir(&state.workspace_root, &session).await {
        Ok(dir) => dir,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error })),
            )
                .into_response();
        }
    };
    let (relative_path, size) =
        match crate::workspace::store_attachment(&dir, &original_name, &bytes).await {
            Ok(stored) => stored,
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": error })),
                )
                    .into_response();
            }
        };
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "name": original_name,
            "path": relative_path,
            "size": size,
        })),
    )
        .into_response()
}

async fn list_sessions(State(state): State<ServerState>) -> Json<Vec<Session>> {
    Json(state.sessions.read().await.clone())
}

async fn create_session(
    State(state): State<ServerState>,
    Json(request): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    if request.title.trim().is_empty() || request.workspace.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "title and workspace are required" })),
        )
            .into_response();
    }
    let id = format!("session-{}", state.next_id.fetch_add(1, Ordering::Relaxed));
    let session = Session {
        id,
        title: request.title,
        workspace: request.workspace,
        created_at: "now".into(),
        updated_at: "now".into(),
    };
    state.sessions.write().await.push(session.clone());
    if let Some(store) = &state.secure_store {
        let sessions = state.sessions.read().await.clone();
        let _ = store.save("sessions", &sessions);
    }
    (StatusCode::CREATED, Json(session)).into_response()
}

async fn stream_events(
    Path(run_id): Path<String>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let events = vec![
        RigaEventEnvelope {
            protocol_version: riga_kernel::PROTOCOL_VERSION,
            event_id: format!("{run_id}-1"),
            session_id: "session-1".into(),
            run_id: run_id.clone(),
            sequence: 1,
            timestamp: "now".into(),
            event: RigaEvent::RunStarted,
        },
        RigaEventEnvelope {
            protocol_version: riga_kernel::PROTOCOL_VERSION,
            event_id: format!("{run_id}-2"),
            session_id: "session-1".into(),
            run_id,
            sequence: 2,
            timestamp: "now".into(),
            event: RigaEvent::RunCompleted {
                output: "transport-ready".into(),
            },
        },
    ];
    let event_stream = stream! {
        for envelope in events {
            let payload = serde_json::to_string(&envelope).expect("event envelope is serializable");
            yield Ok(Event::default().event("riga.event").id(envelope.event_id).data(payload));
        }
    };
    Sse::new(event_stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::{ServerState, router};
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_and_session_routes_are_typed() {
        let app = router(ServerState::default());
        let health = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), 200);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"Test","workspace":"/tmp"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(std::str::from_utf8(&body).unwrap().contains("session-1"));
    }
}
