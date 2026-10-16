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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub provider_type: ws::ProviderType,
}

/// Write payload for a remote provider profile. `api_key` is write-only; an
/// empty value on update preserves the secret already stored for that profile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderProfileInput {
    pub id: Option<String>,
    pub name: String,
    pub provider_type: ws::ProviderType,
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_profile_effort")]
    pub reasoning_effort: String,
    #[serde(default)]
    pub api: ws::ProviderApi,
    #[serde(default)]
    pub subagent_model: Option<String>,
}

/// Safe read model for a saved registration; never includes the API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfileSummary {
    pub id: String,
    pub name: String,
    pub provider_type: ws::ProviderType,
    pub endpoint: String,
    pub model: String,
    pub reasoning_effort: String,
    pub api: ws::ProviderApi,
    pub subagent_model: Option<String>,
    pub api_key_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredProviderProfile {
    id: String,
    name: String,
    config: ws::ProviderConfig,
}

fn default_profile_effort() -> String {
    "low".into()
}

fn profile_summary(profile: &StoredProviderProfile) -> ProviderProfileSummary {
    ProviderProfileSummary {
        id: profile.id.clone(),
        name: profile.name.clone(),
        provider_type: profile.config.provider_type,
        endpoint: profile.config.endpoint.clone(),
        model: profile.config.model.clone(),
        reasoning_effort: profile.config.reasoning_effort.clone(),
        api: profile.config.api,
        subagent_model: profile.config.subagent_model.clone(),
        api_key_configured: !profile.config.api_key.trim().is_empty(),
    }
}

static PROVIDER_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn new_provider_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let count = PROVIDER_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("provider-{nanos:x}-{count:x}")
}

fn normalize_effort(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "medium" => "medium".into(),
        "high" => "high".into(),
        _ => "low".into(),
    }
}

async fn load_provider_profiles() -> Result<Vec<StoredProviderProfile>, String> {
    if let Some(profiles) =
        secure_store::load_json::<Vec<StoredProviderProfile>>("provider_profiles").await?
    {
        return Ok(profiles);
    }
    // One-time migration for installations that have the pre-registry single
    // provider record. The secret remains encrypted and is never returned.
    let Some(mut config) = secure_store::load_json::<ws::ProviderConfig>("provider").await? else {
        return Ok(Vec::new());
    };
    if config.kind == ws::ProviderKind::Local || config.endpoint.trim().is_empty() {
        return Ok(Vec::new());
    }
    let id = config
        .provider_id
        .clone()
        .unwrap_or_else(|| "legacy-default".into());
    config.provider_id = Some(id.clone());
    let profiles = vec![StoredProviderProfile {
        id,
        name: "Default provider".into(),
        config: config.clone(),
    }];
    secure_store::save_json("provider_profiles", &profiles).await?;
    secure_store::save_json("provider", &config).await?;
    Ok(profiles)
}

/// Resolve a saved profile for one explicitly model-routed task. Provider
/// credentials and endpoint are read from the server-side encrypted registry.
pub async fn provider_config_for_profile(
    provider_id: &str,
    model: Option<&str>,
) -> Result<ws::ProviderConfig, String> {
    let profiles = load_provider_profiles().await?;
    let profile = profiles
        .iter()
        .find(|profile| profile.id == provider_id)
        .ok_or_else(|| format!("provider profile `{provider_id}` was not found"))?;
    if profile.config.kind != ws::ProviderKind::Remote {
        return Err("subagents can only select a saved remote provider".into());
    }
    let mut config = profile.config.clone();
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        config.model = model.to_owned();
    }
    Ok(config)
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
        for server in &mut servers {
            for tool in self.state.mcp_runtime.server_tool_names(&server.name).await {
                if !server.tools.contains(&tool) {
                    server.tools.push(tool);
                }
            }
        }
        result.insert(
            "mcp_servers".into(),
            serde_json::to_value(servers).unwrap_or_default(),
        );
        result
    }

    async fn ensure_sessions_loaded(&self) -> Result<(), String> {
        let database_configured = std::env::var("DATABASE_URL")
            .ok()
            .is_some_and(|url| !url.trim().is_empty());
        if let Some(sessions) = secure_store::load_json::<Vec<Session>>("sessions").await? {
            self.state
                .next_id
                .store(next_session_number(&sessions), Ordering::Relaxed);
            *self.state.sessions.write().await = sessions;
        } else if database_configured {
            self.state.sessions.write().await.clear();
        }
        Ok(())
    }

    async fn ensure_transcripts_loaded(&self) -> Result<(), String> {
        let database_configured = std::env::var("DATABASE_URL")
            .ok()
            .is_some_and(|url| !url.trim().is_empty());
        if let Some(transcripts) = secure_store::load_json::<
            std::collections::HashMap<String, Vec<ws::ConversationTurn>>,
        >("transcripts")
        .await?
        {
            *self.state.transcripts.write().await = transcripts;
        } else if database_configured {
            self.state.transcripts.write().await.clear();
        }
        Ok(())
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>, String> {
        self.ensure_sessions_loaded().await?;
        Ok(self.state.sessions.read().await.clone())
    }

    pub async fn session_history(&self, session_id: &str) -> Vec<ws::ConversationTurn> {
        let _ = self.ensure_transcripts_loaded().await;
        let live_history = {
            let transcripts = self.state.transcripts.read().await;
            transcripts.get(session_id).cloned()
        };
        if let Some(history) = live_history
            && !history.is_empty()
        {
            return history;
        }
        let persisted = crate::secure_store::load_json::<
            std::collections::HashMap<String, Vec<ws::ConversationTurn>>,
        >("transcripts")
        .await
        .ok()
        .flatten()
        .and_then(|history| history.get(session_id).cloned())
        .unwrap_or_default();
        if !persisted.is_empty() {
            return persisted;
        }
        journal_history(session_id)
    }

    /// The runs currently executing. Mirrors the WebSocket `ListActiveRuns`
    /// response so the desktop can show and cancel active runs like the browser.
    pub async fn list_active_runs(&self) -> Vec<ws::ActiveRun> {
        self.state
            .runs
            .lock()
            .await
            .iter()
            .map(|(run_id, handle)| ws::ActiveRun {
                run_id: run_id.clone(),
                session_id: handle.session_id.clone(),
                local: handle.local,
            })
            .collect()
    }

    pub async fn create_session(&self, request: CreateSessionRequest) -> Result<Session, String> {
        if request.title.trim().is_empty() || request.workspace.trim().is_empty() {
            return Err("title and workspace are required".into());
        }
        self.ensure_sessions_loaded().await?;
        let session = Session {
            id: format!(
                "session-{}",
                self.state.next_id.fetch_add(1, Ordering::Relaxed)
            ),
            title: request.title,
            workspace: request.workspace,
            archived: false,
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        let mut sessions = self.state.sessions.write().await;
        let mut updated = sessions.clone();
        updated.push(session.clone());
        secure_store::save_json("sessions", &updated).await?;
        *sessions = updated;
        Ok(session)
    }

    pub async fn rename_session(&self, session_id: &str, title: String) -> Result<Session, String> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 80 {
            return Err("title must contain between 1 and 80 characters".into());
        }
        self.ensure_sessions_loaded().await?;
        let mut sessions = self.state.sessions.write().await;
        let Some(index) = sessions.iter().position(|session| session.id == session_id) else {
            return Err("session not found".into());
        };
        let mut updated = sessions.clone();
        updated[index].title = title.to_owned();
        updated[index].updated_at = "now".into();
        let session = updated[index].clone();
        secure_store::save_json("sessions", &updated).await?;
        *sessions = updated;
        Ok(session)
    }

    pub async fn set_session_archived(
        &self,
        session_id: &str,
        archived: bool,
    ) -> Result<Session, String> {
        self.ensure_sessions_loaded().await?;
        let mut sessions = self.state.sessions.write().await;
        let Some(index) = sessions.iter().position(|session| session.id == session_id) else {
            return Err("session not found".into());
        };
        let mut updated = sessions.clone();
        updated[index].archived = archived;
        updated[index].updated_at = "now".into();
        let session = updated[index].clone();
        secure_store::save_json("sessions", &updated).await?;
        *sessions = updated;
        Ok(session)
    }

    pub async fn delete_session(
        &self,
        session_id: &str,
        confirmation_name: &str,
    ) -> Result<(), String> {
        self.ensure_sessions_loaded().await?;
        if self
            .list_active_runs()
            .await
            .iter()
            .any(|run| run.session_id == session_id)
        {
            return Err("cannot delete a session with an active run".into());
        }

        let mut sessions = self.state.sessions.write().await;
        let Some(session) = sessions.iter().find(|session| session.id == session_id) else {
            return Err("session not found".into());
        };
        if session.title != confirmation_name {
            return Err("session name did not match; nothing was deleted".into());
        }
        let original_sessions = sessions.clone();
        let updated_sessions = sessions
            .iter()
            .filter(|session| session.id != session_id)
            .cloned()
            .collect::<Vec<_>>();

        let mut transcripts = self.state.transcripts.write().await;
        let original_transcripts = transcripts.clone();
        let mut updated_transcripts = original_transcripts.clone();
        updated_transcripts.remove(session_id);

        remove_session_journals(session_id)?;
        secure_store::save_json("sessions", &updated_sessions).await?;
        if let Err(error) = secure_store::save_json("transcripts", &updated_transcripts).await {
            let rollback = secure_store::save_json("sessions", &original_sessions).await;
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "could not save deleted conversation history ({error}); restoring session list also failed ({rollback_error})"
                )),
            };
        }
        if let Err(error) = secure_store::remove_provider_session_mappings(session_id).await {
            let sessions_rollback = secure_store::save_json("sessions", &original_sessions).await;
            let transcripts_rollback =
                secure_store::save_json("transcripts", &original_transcripts).await;
            return Err(format!(
                "could not remove provider session mappings ({error}); session rollback: {}; transcript rollback: {}",
                sessions_rollback.err().unwrap_or_else(|| "ok".to_owned()),
                transcripts_rollback
                    .err()
                    .unwrap_or_else(|| "ok".to_owned()),
            ));
        }
        *sessions = updated_sessions;
        *transcripts = updated_transcripts;
        Ok(())
    }

    pub async fn configure_provider(
        &self,
        mut config: ws::ProviderConfig,
    ) -> Result<ProviderConfigured, String> {
        if let Some(provider_id) = config.provider_id.as_deref() {
            let profiles = load_provider_profiles().await?;
            let profile = profiles
                .iter()
                .find(|profile| profile.id == provider_id)
                .ok_or_else(|| format!("provider profile `{provider_id}` was not found"))?;
            // Credentials and connection details always come from the encrypted
            // registration; the client may select a model but cannot substitute
            // a different endpoint under a saved registration ID.
            config.endpoint = profile.config.endpoint.clone();
            config.api_key = profile.config.api_key.clone();
            config.provider_type = profile.config.provider_type;
            config.api = profile.config.api;
            config.kind = ws::ProviderKind::Remote;
            if config.model.trim().is_empty() {
                config.model = profile.config.model.clone();
            }
            if config.reasoning_effort.trim().is_empty() {
                config.reasoning_effort = profile.config.reasoning_effort.clone();
            }
            if config.subagent_model.is_none() {
                config.subagent_model = profile.config.subagent_model.clone();
            }
        } else if config.api_key.trim().is_empty()
            && let Some(existing) =
                secure_store::load_json::<ws::ProviderConfig>("provider").await?
        {
            config.api_key = existing.api_key;
            config.provider_id = existing.provider_id;
            config.provider_type = existing.provider_type;
        }
        // An `opencode_open_ai_compatible` provider is served by the RIGA
        // OpenCode adapter. Start it on demand and default the endpoint to it.
        if config.provider_type == ws::ProviderType::OpencodeOpenAiCompatible
            && let Some(endpoint) = crate::opencode::ensure_local_endpoint(&config.endpoint).await?
        {
            config.endpoint = endpoint;
        }
        secure_store::save_json("provider", &config).await?;
        Ok(ProviderConfigured {
            endpoint: config.endpoint,
            model: config.model,
            reasoning_effort: config.reasoning_effort,
            kind: config.kind,
            api: config.api,
            subagent_model: config.subagent_model,
            provider_id: config.provider_id,
            provider_type: config.provider_type,
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
            provider_id: config.provider_id,
            provider_type: config.provider_type,
        }))
    }

    /// Lists models from the configured OpenAI-compatible provider. Credentials
    /// stay in the server's secure store and are never returned to the TUI.
    pub async fn remote_models(&self) -> Result<Vec<String>, String> {
        let config = secure_store::load_json::<ws::ProviderConfig>("provider")
            .await?
            .ok_or_else(|| "Configure a remote provider in Settings first.".to_string())?;
        fetch_remote_models(&config).await
    }

    pub async fn provider_profiles(&self) -> Result<Vec<ProviderProfileSummary>, String> {
        Ok(load_provider_profiles()
            .await?
            .iter()
            .map(profile_summary)
            .collect())
    }

    pub async fn save_provider_profile(
        &self,
        input: ProviderProfileInput,
    ) -> Result<ProviderProfileSummary, String> {
        let name = input.name.trim();
        let endpoint = input.endpoint.trim();
        if name.is_empty() {
            return Err("Provider name cannot be empty.".into());
        }
        if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
            return Err("Provider endpoint must be an HTTP or HTTPS URL.".into());
        }
        let mut profiles = load_provider_profiles().await?;
        let id = input
            .id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(new_provider_id);
        let old = profiles.iter().find(|profile| profile.id == id).cloned();
        if old.is_none()
            && profiles
                .iter()
                .any(|profile| profile.name.eq_ignore_ascii_case(name))
        {
            return Err(format!("A provider named `{name}` already exists."));
        }
        let api_key = if input.api_key.trim().is_empty() {
            old.as_ref()
                .map(|profile| profile.config.api_key.clone())
                .unwrap_or_default()
        } else {
            input.api_key
        };
        let provider_type = input.provider_type;
        let api = if provider_type == ws::ProviderType::OpencodeOpenAiCompatible {
            ws::ProviderApi::Responses
        } else {
            input.api
        };
        let config = ws::ProviderConfig {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            api_key,
            model: input.model.trim().to_owned(),
            reasoning_effort: normalize_effort(&input.reasoning_effort),
            kind: ws::ProviderKind::Remote,
            api,
            subagent_model: input
                .subagent_model
                .filter(|value| !value.trim().is_empty()),
            provider_id: Some(id.clone()),
            provider_type,
        };
        let profile = StoredProviderProfile {
            id,
            name: name.to_owned(),
            config,
        };
        if let Some(existing) = profiles
            .iter_mut()
            .find(|existing| existing.id == profile.id)
        {
            *existing = profile.clone();
        } else {
            profiles.push(profile.clone());
        }
        secure_store::save_json("provider_profiles", &profiles).await?;
        Ok(profile_summary(&profile))
    }

    pub async fn delete_provider_profile(&self, provider_id: &str) -> Result<(), String> {
        let mut profiles = load_provider_profiles().await?;
        let before = profiles.len();
        profiles.retain(|profile| profile.id != provider_id);
        if profiles.len() == before {
            return Err(format!("provider profile `{provider_id}` was not found"));
        }
        secure_store::save_json("provider_profiles", &profiles).await?;
        if let Some(mut active) = secure_store::load_json::<ws::ProviderConfig>("provider").await?
            && active.provider_id.as_deref() == Some(provider_id)
        {
            active.provider_id = None;
            active.endpoint.clear();
            active.api_key.clear();
            active.model.clear();
            active.kind = ws::ProviderKind::Remote;
            secure_store::save_json("provider", &active).await?;
        }
        Ok(())
    }

    pub async fn remote_models_for(
        &self,
        provider_id: Option<&str>,
    ) -> Result<Vec<String>, String> {
        if let Some(provider_id) = provider_id {
            let profiles = load_provider_profiles().await?;
            let profile = profiles
                .iter()
                .find(|profile| profile.id == provider_id)
                .ok_or_else(|| format!("provider profile `{provider_id}` was not found"))?;
            if profile.config.provider_type == ws::ProviderType::OpencodeOpenAiCompatible
                && let Some(endpoint) =
                    crate::opencode::ensure_local_endpoint(&profile.config.endpoint).await?
            {
                let mut config = profile.config.clone();
                config.endpoint = endpoint;
                return fetch_remote_models(&config).await;
            }
            return fetch_remote_models(&profile.config).await;
        }
        self.remote_models().await
    }

    pub async fn list_mcp_registry(&self) -> Vec<catalog::McpServerSummary> {
        let mut servers = self
            .state
            .mcp_registry
            .read()
            .await
            .iter()
            .map(|record| record.summary.clone())
            .collect::<Vec<_>>();
        for server in &mut servers {
            for tool in self.state.mcp_runtime.server_tool_names(&server.name).await {
                if !server.tools.contains(&tool) {
                    server.tools.push(tool);
                }
            }
        }
        servers
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

    /// Connect every **registered** MCP server to the shared runtime. Runs **at
    /// most once per process**: repeated calls (every run, a reconnect loop) are
    /// no-ops, which is what keeps a host that calls it frequently from spawning
    /// a child-process storm.
    ///
    /// The demo built-in health probes are deliberately **not** connected here.
    /// `riga-health-stdio` spawns `current_exe` with the `mcp-health-stdio` arg;
    /// in a desktop (Tauri) host `current_exe` is the GUI binary, which does not
    /// handle that arg and would launch another app window — a fork bomb. The
    /// WebSocket path (a headless `riga-server`/CLI binary) still uses them.
    pub async fn connect_mcp(&self) {
        self.state
            .mcp_connect_once
            .get_or_init(|| async {
                let records = self.state.mcp_registry.read().await.clone();
                tracing::info!(servers = records.len(), "connecting registered MCP servers");
                self.state.mcp_runtime.connect_records(&records).await;
            })
            .await;
    }

    pub async fn start_run(
        &self,
        run_id: String,
        session_id: String,
        prompt: String,
    ) -> Result<broadcast::Receiver<RigaEventEnvelope>, String> {
        self.ensure_transcripts_loaded().await?;
        // Connect the registered MCP servers (plus the built-in health probes)
        // before the run starts, so their tools are advertised to the model over
        // the IPC path as well as the WebSocket path.
        self.connect_mcp().await;
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
            handle.cancel_notify.notify_waiters();
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

    /// Resolve a session's worktree, creating it from `HEAD` if a run has not.
    async fn session_root(&self, session_id: &str) -> Result<std::path::PathBuf, String> {
        crate::workspace::ensure_worktree(&self.state.workspace_root, session_id).await
    }

    pub async fn workspace_dir(
        &self,
        session_id: &str,
        path: &str,
    ) -> Result<Vec<crate::files::WorkspaceEntry>, String> {
        let root = self.session_root(session_id).await?;
        crate::files::list_dir(&root, path)
    }

    pub async fn workspace_file(
        &self,
        session_id: &str,
        path: &str,
    ) -> Result<crate::files::WorkspaceFile, String> {
        let root = self.session_root(session_id).await?;
        crate::files::read_file(&root, path)
    }

    pub async fn git_status(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::files::GitChange>, String> {
        let root = self.session_root(session_id).await?;
        crate::files::git_status(&root).await
    }

    pub async fn git_diff(
        &self,
        session_id: &str,
        path: &str,
    ) -> Result<crate::files::WorkspaceDiff, String> {
        let root = self.session_root(session_id).await?;
        crate::files::git_diff(&root, path).await
    }

    pub async fn git_commit(
        &self,
        session_id: &str,
        message: &str,
    ) -> Result<crate::files::GitCommitResult, String> {
        let root = self.session_root(session_id).await?;
        crate::files::git_commit(&root, message).await
    }

    pub async fn git_push(&self, session_id: &str) -> Result<crate::files::GitPushResult, String> {
        let root = self.session_root(session_id).await?;
        crate::files::git_push(&root).await
    }

    pub async fn git_log(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::files::GitCommit>, String> {
        let root = self.session_root(session_id).await?;
        crate::files::git_log(&root, limit).await
    }

    pub async fn git_commit_diff(
        &self,
        session_id: &str,
        commit: &str,
    ) -> Result<crate::files::WorkspaceDiff, String> {
        let root = self.session_root(session_id).await?;
        crate::files::git_commit_diff(&root, commit).await
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
                session_id: session_id.into(),
                events: events.clone(),
                cancel: cancel.clone(),
                cancel_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
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

fn remote_models_url(endpoint: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(endpoint.trim())
        .map_err(|_| "Provider endpoint must be a valid HTTP(S) URL.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Provider endpoint must use HTTP or HTTPS.".into());
    }
    let path = url.path().trim_end_matches('/');
    let base_path = ["/chat/completions", "/responses", "/models"]
        .into_iter()
        .find_map(|suffix| path.strip_suffix(suffix))
        .unwrap_or(path);
    let models_path = if base_path.is_empty() {
        "/models".to_string()
    } else {
        format!("{base_path}/models")
    };
    url.set_path(&models_path);
    Ok(url)
}

fn parse_remote_model_ids(value: &serde_json::Value) -> Result<Vec<String>, String> {
    let records = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| "Provider model list had an unsupported response shape.".to_string())?;
    let mut ids = records
        .iter()
        .filter_map(|record| {
            record
                .as_str()
                .or_else(|| {
                    record
                        .get("id")
                        .or_else(|| record.get("name"))
                        .and_then(serde_json::Value::as_str)
                })
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

async fn fetch_remote_models(config: &ws::ProviderConfig) -> Result<Vec<String>, String> {
    if config.endpoint.trim().is_empty() {
        return Err("Configure a provider endpoint in Settings first.".into());
    }
    let url = remote_models_url(&config.endpoint)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| format!("Could not prepare provider request: {error}"))?;
    let mut request = client.get(url);
    if !config.api_key.trim().is_empty() {
        request = request.bearer_auth(&config.api_key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not load remote models: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Provider rejected the model-list request: {error}"))?;
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("Could not read provider model list: {error}"))?;
    parse_remote_model_ids(&value)
}

fn run_journal_path(run_id: &str) -> PathBuf {
    secure_store::data_root()
        .join("runs")
        .join(format!("run-{}.json", sanitize_run_id(run_id)))
}

fn remove_session_journals(session_id: &str) -> Result<(), String> {
    let runs_dir = secure_store::data_root().join("runs");
    let entries = match std::fs::read_dir(&runs_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("could not inspect run journals: {error}")),
    };
    let mut journals = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not inspect run journal: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(events) = serde_json::from_slice::<Vec<RigaEventEnvelope>>(&bytes) else {
            continue;
        };
        if events.iter().any(|event| event.session_id == session_id) {
            journals.push(path);
        }
    }
    for journal in journals {
        std::fs::remove_file(&journal).map_err(|error| {
            format!(
                "could not remove run journal {}: {error}",
                journal.display()
            )
        })?;
    }
    Ok(())
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

fn next_session_number(sessions: &[Session]) -> u64 {
    sessions
        .iter()
        .filter_map(|session| session.id.strip_prefix("session-")?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn journal_history(session_id: &str) -> Vec<ws::ConversationTurn> {
    let runs_dir = secure_store::data_root().join("runs");
    let Ok(entries) = std::fs::read_dir(runs_dir) else {
        return Vec::new();
    };
    let mut runs = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|value| value.to_str()) == Some("json"))
                .then(|| riga_kernel::persistence::EventJournal::open(path).ok())
                .flatten()
        })
        .filter_map(|journal| {
            let events = journal.all();
            let first = events.first()?;
            if first.session_id != session_id {
                return None;
            }
            let mut output = String::new();
            for event in events {
                if let riga_kernel::events::RigaEvent::TextDelta { delta } = &event.event {
                    output.push_str(delta);
                }
            }
            (!output.is_empty()).then(|| (first.timestamp.clone(), output))
        })
        .collect::<Vec<_>>();
    runs.sort_by(|left, right| left.0.cmp(&right.0));
    runs.into_iter()
        .map(|(_, content)| ws::ConversationTurn {
            role: "assistant".into(),
            content,
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
        assert_eq!(
            service.list_sessions().await.unwrap().last().unwrap().id,
            session.id
        );
    }

    #[tokio::test]
    async fn session_history_reads_live_transcripts_for_selected_session() {
        let service = IpcService::new(ServerState::default());
        service.state.transcripts.write().await.insert(
            "session-history".into(),
            vec![
                ws::ConversationTurn {
                    role: "user".into(),
                    content: "hello from history".into(),
                },
                ws::ConversationTurn {
                    role: "assistant".into(),
                    content: "welcome back".into(),
                },
            ],
        );

        let history = service.session_history("session-history").await;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "hello from history");
        assert_eq!(history[1].content, "welcome back");
    }

    #[tokio::test]
    async fn remote_model_discovery_uses_provider_endpoint_and_bearer_auth() {
        let auth_seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let route_auth_seen = auth_seen.clone();
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let auth_seen = route_auth_seen.clone();
                async move {
                    auth_seen.store(
                        headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            == Some("Bearer test-key"),
                        Ordering::Relaxed,
                    );
                    axum::Json(serde_json::json!({
                        "data": [{"id": "zeta-model"}, {"id": "alpha-model"}]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let config = ws::ProviderConfig {
            endpoint: format!("http://{address}/v1/chat/completions"),
            api_key: "test-key".into(),
            model: "current-model".into(),
            reasoning_effort: "low".into(),
            kind: ws::ProviderKind::Remote,
            api: ws::ProviderApi::Chat,
            subagent_model: None,
            provider_id: None,
            provider_type: ws::ProviderType::OpenAiCompatible,
        };

        let models = fetch_remote_models(&config).await.unwrap();
        server.abort();

        assert_eq!(models, vec!["alpha-model", "zeta-model"]);
        assert!(auth_seen.load(Ordering::Relaxed));
    }

    #[test]
    fn remote_model_response_parser_accepts_names_and_deduplicates_ids() {
        let models = parse_remote_model_ids(&serde_json::json!({
            "models": [{"name": "gemini-pro"}, "small", {"id": "small"}]
        }))
        .unwrap();
        assert_eq!(models, vec!["gemini-pro", "small"]);
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
