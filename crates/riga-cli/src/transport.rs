use riga_kernel::{events::RigaEventEnvelope, state::Session};
use riga_server::{
    CreateSessionRequest, HealthResponse, LocalModelOverview, ServerState,
    catalog::McpServerSummary,
    ipc::{Attachment, IpcAttachment, IpcService, ProviderConfigured},
    local_model::LocalModelEvent,
    ws::{ActiveRun, ProviderConfig},
};
use tokio::sync::broadcast;

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
