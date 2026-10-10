use std::{future::Future, pin::Pin};

use riga_kernel::{events::RigaEventEnvelope, state::Session};
use riga_server::{
    CreateSessionRequest, HealthResponse, LocalModelOverview, ServerState,
    catalog::McpServerSummary,
    ipc::{Attachment, IpcAttachment, IpcService, ProviderConfigured},
    local_model::LocalModelEvent,
    ws::{ActiveRun, ProviderConfig},
};
use tokio::sync::broadcast;

pub type TransportFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Transport boundary consumed by the application layer. IPC is the default
/// implementation; a WebSocket adapter can implement the same semantic seam
/// without changing projection or rendering code.
#[allow(dead_code)]
pub trait RigaTransport: Clone + Send + Sync + 'static {
    fn subscribe_local_models(&self) -> broadcast::Receiver<LocalModelEvent>;
    fn local_models(&self) -> TransportFuture<LocalModelOverview>;
    fn session_history(
        &self,
        session_id: String,
    ) -> TransportFuture<Vec<riga_server::ws::ConversationTurn>>;

    fn local_model_action(
        &self,
        action: String,
        model_id: Option<String>,
        path: Option<String>,
    ) -> TransportFuture<Result<(), String>>;

    fn upload_attachment(
        &self,
        attachment: IpcAttachment,
    ) -> TransportFuture<Result<Attachment, String>>;

    fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> TransportFuture<Result<Session, String>>;

    fn configure_provider(
        &self,
        config: ProviderConfig,
    ) -> TransportFuture<Result<ProviderConfigured, String>>;

    fn start_run(
        &self,
        run_id: String,
        session_id: String,
        prompt: String,
    ) -> TransportFuture<Result<broadcast::Receiver<RigaEventEnvelope>, String>>;

    fn subscribe_run(
        &self,
        run_id: String,
        after_sequence: u64,
    ) -> TransportFuture<Result<broadcast::Receiver<RigaEventEnvelope>, String>>;

    fn cancel_run(&self, run_id: String) -> TransportFuture<bool>;

    fn respond_to_approval(
        &self,
        approval_id: String,
        approved: bool,
        always: bool,
    ) -> TransportFuture<bool>;
}

/// The default CLI transport. It deliberately delegates to the same in-process
/// service used by the Tauri adapter instead of starting a second server.
#[derive(Clone)]
pub struct IpcTransport {
    service: IpcService,
}

#[allow(dead_code)]
impl IpcTransport {
    pub fn new(state: ServerState) -> Self {
        Self {
            service: IpcService::new(state),
        }
    }

    pub fn health(&self) -> HealthResponse {
        self.service.health()
    }

    pub async fn catalog(&self) -> std::collections::BTreeMap<String, serde_json::Value> {
        self.service.catalog().await
    }

    pub async fn list_sessions(&self) -> Vec<Session> {
        self.service.list_sessions().await
    }

    pub async fn session_history(
        &self,
        session_id: &str,
    ) -> Vec<riga_server::ws::ConversationTurn> {
        self.service.session_history(session_id).await
    }

    pub async fn create_session(&self, request: CreateSessionRequest) -> Result<Session, String> {
        self.service.create_session(request).await
    }

    pub async fn provider(&self) -> Result<Option<ProviderConfigured>, String> {
        self.service.provider().await
    }

    pub async fn configure_provider(
        &self,
        config: ProviderConfig,
    ) -> Result<ProviderConfigured, String> {
        self.service.configure_provider(config).await
    }

    pub async fn start_run(
        &self,
        run_id: String,
        session_id: String,
        prompt: String,
    ) -> Result<broadcast::Receiver<RigaEventEnvelope>, String> {
        self.service.start_run(run_id, session_id, prompt).await
    }

    pub async fn subscribe_run(
        &self,
        run_id: &str,
        after_sequence: u64,
    ) -> Result<broadcast::Receiver<RigaEventEnvelope>, String> {
        self.service.subscribe_run(run_id, after_sequence).await
    }

    pub async fn cancel_run(&self, run_id: &str) -> bool {
        self.service.cancel_run(run_id).await
    }

    pub async fn list_active_runs(&self) -> Vec<ActiveRun> {
        self.service.list_active_runs().await
    }

    pub async fn respond_to_approval(
        &self,
        approval_id: &str,
        approved: bool,
        always: bool,
    ) -> bool {
        self.service
            .respond_to_approval(approval_id, approved, always)
            .await
    }

    pub async fn mcp_registry(&self) -> Vec<McpServerSummary> {
        self.service.list_mcp_registry().await
    }

    pub async fn local_models(&self) -> LocalModelOverview {
        self.service.local_models().await
    }

    pub fn subscribe_local_models(&self) -> broadcast::Receiver<LocalModelEvent> {
        self.service.subscribe_local_models()
    }

    pub async fn local_model_action(
        &self,
        action: &str,
        model_id: Option<&str>,
        path: Option<&str>,
    ) -> Result<(), String> {
        self.service
            .local_model_action(action, model_id, path)
            .await
    }

    pub async fn upload_attachment(&self, attachment: IpcAttachment) -> Result<Attachment, String> {
        self.service.upload_attachment(attachment).await
    }
}

impl RigaTransport for IpcTransport {
    fn subscribe_local_models(&self) -> broadcast::Receiver<LocalModelEvent> {
        self.subscribe_local_models()
    }

    fn local_models(&self) -> TransportFuture<LocalModelOverview> {
        let transport = self.clone();
        Box::pin(async move { transport.local_models().await })
    }

    fn session_history(
        &self,
        session_id: String,
    ) -> TransportFuture<Vec<riga_server::ws::ConversationTurn>> {
        let transport = self.clone();
        Box::pin(async move { transport.session_history(&session_id).await })
    }

    fn local_model_action(
        &self,
        action: String,
        model_id: Option<String>,
        path: Option<String>,
    ) -> TransportFuture<Result<(), String>> {
        let transport = self.clone();
        Box::pin(async move {
            transport
                .local_model_action(action.as_str(), model_id.as_deref(), path.as_deref())
                .await
        })
    }

    fn upload_attachment(
        &self,
        attachment: IpcAttachment,
    ) -> TransportFuture<Result<Attachment, String>> {
        let transport = self.clone();
        Box::pin(async move { transport.upload_attachment(attachment).await })
    }

    fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> TransportFuture<Result<Session, String>> {
        let transport = self.clone();
        Box::pin(async move { transport.create_session(request).await })
    }

    fn configure_provider(
        &self,
        config: ProviderConfig,
    ) -> TransportFuture<Result<ProviderConfigured, String>> {
        let transport = self.clone();
        Box::pin(async move { transport.configure_provider(config).await })
    }

    fn start_run(
        &self,
        run_id: String,
        session_id: String,
        prompt: String,
    ) -> TransportFuture<Result<broadcast::Receiver<RigaEventEnvelope>, String>> {
        let transport = self.clone();
        Box::pin(async move { transport.start_run(run_id, session_id, prompt).await })
    }

    fn subscribe_run(
        &self,
        run_id: String,
        after_sequence: u64,
    ) -> TransportFuture<Result<broadcast::Receiver<RigaEventEnvelope>, String>> {
        let transport = self.clone();
        Box::pin(async move { transport.subscribe_run(&run_id, after_sequence).await })
    }

    fn cancel_run(&self, run_id: String) -> TransportFuture<bool> {
        let transport = self.clone();
        Box::pin(async move { transport.cancel_run(&run_id).await })
    }

    fn respond_to_approval(
        &self,
        approval_id: String,
        approved: bool,
        always: bool,
    ) -> TransportFuture<bool> {
        let transport = self.clone();
        Box::pin(async move {
            transport
                .respond_to_approval(&approval_id, approved, always)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::IpcTransport;
    use riga_server::{CreateSessionRequest, ServerState};

    #[tokio::test]
    async fn default_transport_uses_one_in_process_server_state() {
        let transport = IpcTransport::new(ServerState::default());
        assert_eq!(transport.health().protocol_version, 1);
        assert_eq!(transport.health().adapter, "riga-server");

        let session = transport
            .create_session(CreateSessionRequest {
                title: "CLI test".into(),
                workspace: ".".into(),
            })
            .await
            .expect("session creation should use IPC");
        assert!(
            transport
                .list_sessions()
                .await
                .iter()
                .any(|item| item.id == session.id)
        );
    }

    #[tokio::test]
    async fn active_run_and_approval_operations_are_safe_when_empty() {
        let transport = IpcTransport::new(ServerState::default());
        assert!(transport.list_active_runs().await.is_empty());
        assert!(!transport.cancel_run("missing-run").await);
        assert!(
            !transport
                .respond_to_approval("missing-approval", true, false)
                .await
        );
    }
}
