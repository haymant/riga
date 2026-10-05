#![doc = "RIGA HTTP/SSE transport adapter."]

use std::path::PathBuf;

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_stream::stream;
use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
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

pub mod catalog;
pub mod secure_store;
pub mod ws;

pub const ADAPTER_NAME: &str = "riga-server";

#[derive(Clone)]
pub struct ServerState {
    sessions: Arc<RwLock<Vec<Session>>>,
    next_id: Arc<AtomicU64>,
    pub(crate) workspace_root: PathBuf,
    pub(crate) secure_store: Option<Arc<secure_store::SecureStore>>,
}

impl Default for ServerState {
    fn default() -> Self {
        let secure_store = secure_store::SecureStore::from_env().map(Arc::new);
        let sessions = secure_store
            .as_ref()
            .and_then(|store| store.load::<Vec<Session>>("sessions").ok().flatten())
            .unwrap_or_default();
        Self {
            sessions: Arc::new(RwLock::new(sessions)),
            next_id: Arc::new(AtomicU64::new(1)),
            workspace_root: catalog::workspace_root(),
            secure_store,
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
        .route("/attachments", post(upload_attachment))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/runs/{run_id}/events", get(stream_events))
        .route("/ws", get(ws_upgrade))
        .with_state(state)
}

async fn ws_upgrade(
    State(state): State<ServerState>,
    upgrade: axum::extract::ws::WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| ws::upgrade(socket, state.workspace_root.clone()))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        protocol_version: riga_kernel::PROTOCOL_VERSION,
        adapter: ADAPTER_NAME,
        persistence_backend: secure_store::database_backend(),
    })
}

async fn tool_catalog(
    State(state): State<ServerState>,
) -> Json<std::collections::BTreeMap<String, serde_json::Value>> {
    Json(catalog::catalog(&state.workspace_root))
}

async fn upload_attachment(
    State(state): State<ServerState>,
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
    let safe_name = original_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let safe_name = if safe_name.is_empty() {
        "attachment".to_owned()
    } else {
        safe_name
    };
    let relative_path = format!(
        "tmp/riga-attachments/{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default(),
        safe_name
    );
    let path = state.workspace_root.join(&relative_path);
    if let Some(parent) = path.parent()
        && let Err(error) = tokio::fs::create_dir_all(parent).await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response();
    }
    if let Err(error) = tokio::fs::write(&path, &bytes).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response();
    }
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "name": original_name,
            "path": relative_path,
            "size": bytes.len(),
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
