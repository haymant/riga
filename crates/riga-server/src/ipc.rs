use std::{collections::BTreeMap, path::PathBuf, sync::atomic::Ordering};

use riga_kernel::{events::RigaEventEnvelope, state::Session};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{
    CreateSessionRequest, HealthResponse, LocalModelOverview, ServerState, catalog, secure_store,
    ws,
};

/// In-process server API used by Tauri commands. It deliberately delegates run
/// execution to the same `ws` provider pipeline used by the HTTP adapter.
#[derive(Clone)]
pub struct IpcService {
    state: ServerState,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderConfigured {
    pub endpoint: String,
    pub model: String,
    pub reasoning_effort: String,
    pub kind: ws::ProviderKind,
    pub api: ws::ProviderApi,
    pub subagent_model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Attachment {
    pub name: String,
    pub path: String,
    pub size: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IpcAttachment {
    pub name: String,
    pub bytes: Vec<u8>,
    /// Session whose worktree receives the file. Defaults to the base workspace
    /// when absent, for a client that predates session-scoped uploads.
    #[serde(default)]
    pub session_id: Option<String>,
}

impl IpcService {
    pub fn new(state: ServerState) -> Self {
        Self { state }
    }

    pub fn state(&self) -> ServerState {
        self.state.clone()
    }

    pub fn health(&self) -> HealthResponse {
        HealthResponse {
            protocol_version: riga_kernel::PROTOCOL_VERSION,
            adapter: crate::ADAPTER_NAME,
            persistence_backend: secure_store::database_backend(),
        }
    }

    pub async fn catalog(&self) -> BTreeMap<String, serde_json::Value> {
        let mut result = catalog::catalog(&self.state.workspace_root);
        let mut servers = vec![
            catalog::builtin_health_stdio(),
            catalog::builtin_health_http(),
        ];
        servers.extend(
            self.state
                .mcp_registry
                .read()
                .await
                .iter()
                .map(|record| record.summary.clone()),
        );
        result.insert(
            "mcp_servers".into(),
            serde_json::to_value(servers).unwrap_or_default(),
        );
        result
    }

    pub async fn list_sessions(&self) -> Vec<Session> {
        self.state.sessions.read().await.clone()
    }

    pub async fn create_session(&self, request: CreateSessionRequest) -> Result<Session, String> {
        if request.title.trim().is_empty() || request.workspace.trim().is_empty() {
            return Err("title and workspace are required".into());
        }
        let session = Session {
            id: format!(
                "session-{}",
                self.state.next_id.fetch_add(1, Ordering::Relaxed)
            ),
            title: request.title,
            workspace: request.workspace,
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        self.state.sessions.write().await.push(session.clone());
        if let Some(store) = &self.state.secure_store {
            let sessions = self.state.sessions.read().await.clone();
            store
                .save("sessions", &sessions)
                .map_err(|error| error.to_string())?;
        }
        Ok(session)
    }

    pub async fn configure_provider(
        &self,
        mut config: ws::ProviderConfig,
    ) -> Result<ProviderConfigured, String> {
        if config.api_key.trim().is_empty()
            && let Some(existing) =
                secure_store::load_json::<ws::ProviderConfig>("provider").await?
        {
            config.api_key = existing.api_key;
        }
        secure_store::save_json("provider", &config).await?;
        Ok(ProviderConfigured {
            endpoint: config.endpoint,
            model: config.model,
            reasoning_effort: config.reasoning_effort,
            kind: config.kind,
            api: config.api,
            subagent_model: config.subagent_model,
        })
    }

    /// The provider currently stored, so the settings form can be restored on
    /// connect. The API key is intentionally omitted: it is write-only from the
    /// UI and is reused server-side when a later save leaves it blank.
    pub async fn provider(&self) -> Result<Option<ProviderConfigured>, String> {
        let Some(config) = secure_store::load_json::<ws::ProviderConfig>("provider").await? else {
            return Ok(None);
        };
        Ok(Some(ProviderConfigured {
            endpoint: config.endpoint,
            model: config.model,
            reasoning_effort: config.reasoning_effort,
            kind: config.kind,
            api: config.api,
            subagent_model: config.subagent_model,
        }))
    }

    pub async fn list_mcp_registry(&self) -> Vec<catalog::McpServerSummary> {
        self.state
            .mcp_registry
            .read()
            .await
            .iter()
            .map(|record| record.summary.clone())
            .collect()
    }

    pub async fn save_mcp_registry(
        &self,
        request: crate::McpRegistryRequest,
    ) -> Result<Vec<catalog::McpServerSummary>, String> {
        let name = request.name.trim();
        if name.is_empty() || !matches!(request.transport.as_str(), "stdio" | "http") {
            return Err("name and transport (stdio or http) are required".into());
        }
        if request.transport == "http" && request.url.as_deref().unwrap_or("").trim().is_empty() {
            return Err("HTTP stream URL is required".into());
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
            summary,
            api_key: request.api_key.filter(|key| !key.is_empty()),
        };
        let mut registry = self.state.mcp_registry.write().await;
        registry.retain(|existing| existing.summary.name != record.summary.name);
        registry.push(record);
        let snapshot = registry.clone();
        drop(registry);
        if let Some(store) = &self.state.secure_store {
            store
                .save("mcp_registry", &snapshot)
                .map_err(|error| error.to_string())?;
        } else {
            std::fs::write(
                self.state.workspace_root.join(".riga-mcp-registry.json"),
                serde_json::to_vec_pretty(&snapshot).unwrap_or_default(),
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(snapshot.into_iter().map(|record| record.summary).collect())
    }

    pub fn subscribe_local_models(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::local_model::LocalModelEvent> {
        self.state.local_models.subscribe()
    }

    pub async fn start_run(
        &self,
        run_id: String,
        session_id: String,
        prompt: String,
    ) -> Result<broadcast::Receiver<RigaEventEnvelope>, String> {
        ws::start_ipc_run(
            self.state.workspace_root.clone(),
            self.state.mcp_runtime.clone(),
            self.state.local_models.clone(),
            self.state.transcripts.clone(),
            self.state.secure_store.clone(),
            self.state.approvals.clone(),
            self.state.runs.clone(),
            run_id,
            session_id,
            prompt,
        )
        .await
    }

    pub async fn subscribe_run(
        &self,
        run_id: &str,
        after_sequence: u64,
    ) -> Result<broadcast::Receiver<RigaEventEnvelope>, String> {
        if let Some(handle) = self.state.runs.lock().await.get(run_id) {
            return Ok(handle.events.subscribe());
        }
        let journal = riga_kernel::persistence::EventJournal::open(run_journal_path(run_id))
            .map_err(|error| error.message)?;
        let (sender, receiver) = broadcast::channel(1024);
        for event in journal.after_sequence(after_sequence) {
            let _ = sender.send(event);
        }
        Ok(receiver)
    }

    pub async fn cancel_run(&self, run_id: &str) -> bool {
        if let Some(handle) = self.state.runs.lock().await.get(run_id) {
            handle.cancel.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub async fn respond_to_approval(
        &self,
        approval_id: &str,
        approved: bool,
        always: bool,
    ) -> bool {
        self.state
            .approvals
            .resolve(approval_id, ws::ApprovalReply { approved, always })
            .await
    }

    pub async fn local_models(&self) -> LocalModelOverview {
        LocalModelOverview {
            accelerator: crate::local_model::accelerator_label(),
            catalog: self.state.local_models.curated_catalog(),
            installed: self.state.local_models.list_installed().unwrap_or_default(),
            loaded: self.state.local_models.loaded_file_name(),
        }
    }

    pub async fn local_model_action(
        &self,
        action: &str,
        model_id: Option<&str>,
        path: Option<&str>,
    ) -> Result<(), String> {
        match action {
            "download" => {
                let model_id = model_id.ok_or("model_id is required")?;
                // The transfer outlives this command, exactly as it does over
                // HTTP. Awaiting it here would keep the invoke promise — and so
                // the UI's busy state — pending for the whole multi-gigabyte
                // download, which also leaves the Cancel button disabled.
                // Reject unknown or duplicate ids first, then detach and report
                // through the local-model event stream.
                if let Some(blocker) = self.state.local_models.download_blocker(model_id) {
                    return Err(blocker);
                }
                let runtime = self.state.local_models.clone();
                let model_id = model_id.to_owned();
                tokio::spawn(async move {
                    if let Err(error) = runtime.start_download(&model_id).await {
                        tracing::error!(model_id = %model_id, %error, "local model download failed");
                    }
                });
                Ok(())
            }
            "cancel" => self
                .state
                .local_models
                .cancel_download(model_id.ok_or("model_id is required")?),
            "load" => {
                self.state
                    .local_models
                    .load_model(path.ok_or("path is required")?)
                    .await
            }
            "unload" => self.state.local_models.unload_model(),
            other => Err(format!("unknown local model action: {other}")),
        }
    }

    pub async fn upload_attachment(&self, attachment: IpcAttachment) -> Result<Attachment, String> {
        // Store inside the session's worktree so the agent's tools find the file
        // at the relative path the prompt advertises.
        let session = attachment.session_id.as_deref().unwrap_or_default();
        let dir =
            crate::workspace::ensure_attachment_dir(&self.state.workspace_root, session).await?;
        let (path, size) =
            crate::workspace::store_attachment(&dir, &attachment.name, &attachment.bytes).await?;
        Ok(Attachment {
            name: attachment.name,
            path,
            size,
        })
    }

    #[cfg(test)]
    async fn start_deterministic_test_run(
        &self,
        run_id: &str,
        session_id: &str,
    ) -> broadcast::Receiver<RigaEventEnvelope> {
        let (events, _) = broadcast::channel(32);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.state.runs.lock().await.insert(
            run_id.into(),
            ws::RunHandle {
                events: events.clone(),
                cancel: cancel.clone(),
                local: false,
            },
        );
        let run_id = run_id.to_owned();
        let session_id = session_id.to_owned();
        let task_run_id = run_id.clone();
        let runs = self.state.runs.clone();
        tokio::spawn(async move {
            let mut channel =
                ws::RunChannel::without_journal(&task_run_id, &session_id, events.clone());
            channel.emit(riga_kernel::events::RigaEvent::RunStarted);
            for delta in ["deterministic", "-ipc"] {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                channel.emit(riga_kernel::events::RigaEvent::TextDelta {
                    delta: delta.into(),
                });
                tokio::task::yield_now().await;
            }
            if !cancel.load(Ordering::Relaxed) {
                channel.emit(riga_kernel::events::RigaEvent::RunCompleted {
                    output: "deterministic-ipc".into(),
                });
            }
            runs.lock().await.remove(&task_run_id);
        });
        self.state
            .runs
            .lock()
            .await
            .get(&run_id)
            .unwrap()
            .events
            .subscribe()
    }
}

fn run_journal_path(run_id: &str) -> PathBuf {
    secure_store::data_root()
        .join("runs")
        .join(format!("run-{}.json", sanitize_run_id(run_id)))
}

fn sanitize_run_id(run_id: &str) -> String {
    run_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ipc_health_catalog_and_sessions_use_one_server_state() {
        let service = IpcService::new(ServerState::default());
        assert_eq!(service.health().adapter, "riga-server");
        assert!(service.catalog().await.contains_key("tools"));
        let session = service
            .create_session(CreateSessionRequest {
                title: "IPC".into(),
                workspace: ".".into(),
            })
            .await
            .unwrap();
        assert_eq!(service.list_sessions().await.last().unwrap().id, session.id);
    }

    #[tokio::test]
    async fn ipc_run_events_are_ordered_and_cancel_can_be_resumed_without_http() {
        let service = IpcService::new(ServerState::default());
        let mut events = service
            .start_deterministic_test_run("ipc-run", "session-1")
            .await;
        let mut resumed = service.subscribe_run("ipc-run", 0).await.unwrap();
        let started = events.recv().await.unwrap();
        assert_eq!(started.sequence, 1);
        assert!(service.cancel_run("ipc-run").await);
        assert!(resumed.recv().await.is_ok());
    }
}
