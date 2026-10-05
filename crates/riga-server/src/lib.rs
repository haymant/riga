#![doc = "RIGA HTTP/SSE transport adapter."]

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_stream::stream;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use riga_kernel::{
    events::{RigaEvent, RigaEventEnvelope},
    state::Session,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

pub mod ws;

pub const ADAPTER_NAME: &str = "riga-server";

#[derive(Clone)]
pub struct ServerState {
    sessions: Arc<RwLock<Vec<Session>>>,
    next_id: Arc<AtomicU64>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(Vec::new())),
            next_id: Arc::new(AtomicU64::new(1)),
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
}

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/runs/{run_id}/events", get(stream_events))
        .route("/ws", get(ws_upgrade))
        .with_state(state)
}

async fn ws_upgrade(upgrade: axum::extract::ws::WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(ws::upgrade)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        protocol_version: riga_kernel::PROTOCOL_VERSION,
        adapter: ADAPTER_NAME,
    })
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
