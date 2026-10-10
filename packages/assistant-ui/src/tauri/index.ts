import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Attachment,
  ActiveRun,
  Catalog,
  CreateSessionRequest,
  Health,
  McpRegistryRequest,
  McpServerSummary,
  RigaEventEnvelope,
  RigaTransport,
  RigaTransportListeners,
  Session,
  ConversationTurn,
} from "../protocol";

export const RIGA_TAURI_TRANSPORT_VERSION = "0.1.0" as const;

export const RIGA_IPC_COMMANDS = {
  connect: "riga_transport_connect",
  disconnect: "riga_transport_disconnect",
  health: "riga_health",
  catalog: "riga_catalog",
  listSessions: "riga_list_sessions",
  createSession: "riga_create_session",
  sessionHistory: "riga_session_history",
  renameSession: "riga_rename_session",
  listActiveRuns: "riga_list_active_runs",
  configureProvider: "riga_configure_provider",
  startRun: "riga_start_run",
  resumeRun: "riga_resume_run",
  cancelRun: "riga_cancel_run",
  approval: "riga_respond_to_approval",
  listMcpRegistry: "riga_list_mcp_registry",
  saveMcpRegistry: "riga_save_mcp_registry",
  uploadAttachment: "riga_upload_attachment",
  localModels: "riga_local_models",
  localModelAction: "riga_local_model_action",
} as const;

export const RIGA_IPC_EVENTS = {
  run: "riga://run-event",
  localModel: "riga://local-model-event",
  providerConfigured: "riga://provider-configured",
  error: "riga://transport-error",
} as const;

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, handler: (event: { payload: T }) => void) => Promise<UnlistenFn>;
type TransportError = { code: string; message: string };
type ProviderConfigured = {
  endpoint: string;
  model: string;
  reasoning_effort: "low" | "medium" | "high";
  kind?: "remote" | "local";
  api?: "chat" | "responses";
  subagent_model?: string;
};

export type TauriTransportOptions = {
  invoke?: Invoke;
  listen?: Listen;
  listeners?: RigaTransportListeners;
};

export class RigaTauriTransport implements RigaTransport {
  private readonly invoke: Invoke;
  private readonly listen: Listen;
  private readonly listeners: RigaTransportListeners;
  private unlisten: UnlistenFn[] = [];
  private connected = false;
  private connecting: Promise<void> | null = null;

  constructor(options: TauriTransportOptions = {}) {
    this.invoke = options.invoke ?? ((command, args) => tauriInvoke(command, args));
    this.listen = options.listen ?? ((event, handler) => tauriListen(event, handler));
    this.listeners = options.listeners ?? {};
  }

  async connect(): Promise<void> {
    if (this.connected) return;
    if (this.connecting) return this.connecting;
    this.connecting = this.open().finally(() => { this.connecting = null; });
    return this.connecting;
  }

  wake(): void {
    if (!this.connected) void this.connect();
  }

  close(): void {
    this.connected = false;
    for (const unsubscribe of this.unlisten.splice(0)) unsubscribe();
    void this.invoke<void>(RIGA_IPC_COMMANDS.disconnect).catch(() => undefined);
  }

  health(): Promise<Health> { return this.call(RIGA_IPC_COMMANDS.health); }
  catalog(): Promise<Catalog> { return this.call(RIGA_IPC_COMMANDS.catalog); }
  listSessions(): Promise<Session[]> { return this.call(RIGA_IPC_COMMANDS.listSessions); }
  createSession(request: CreateSessionRequest): Promise<Session> { return this.call(RIGA_IPC_COMMANDS.createSession, { request }); }
  sessionHistory(sessionId: string): Promise<ConversationTurn[]> { return this.call(RIGA_IPC_COMMANDS.sessionHistory, { sessionId }); }
  renameSession(sessionId: string, title: string): Promise<Session> { return this.call(RIGA_IPC_COMMANDS.renameSession, { sessionId, title }); }
  // Tauri maps camelCase JS keys to the command's snake_case parameters, so the
  // flat command args are camelCase. A parameter that is itself a struct is
  // passed under its own key (`request` / `attachment`) and keeps serde's
  // snake_case field names inside.
  configureProvider(endpoint: string, apiKey: string, model: string, reasoningEffort: "low" | "medium" | "high", kind = "remote", api = "chat", subagentModel = ""): Promise<void> {
    return this.call(RIGA_IPC_COMMANDS.configureProvider, { endpoint, apiKey, model, reasoningEffort, kind, api, subagentModel });
  }
  startRun(runId: string, sessionId: string, prompt: string): Promise<void> {
    return this.call(RIGA_IPC_COMMANDS.startRun, { runId, sessionId, prompt });
  }
  resumeRun(runId: string, afterSequence: number): Promise<void> {
    return this.call(RIGA_IPC_COMMANDS.resumeRun, { runId, afterSequence });
  }
  cancelRun(runId: string): Promise<void> { return this.call(RIGA_IPC_COMMANDS.cancelRun, { runId }); }
  listActiveRuns(): Promise<ActiveRun[]> { return this.call(RIGA_IPC_COMMANDS.listActiveRuns); }
  respondToApproval(runId: string, approvalId: string, approved: boolean, option?: "once" | "always"): Promise<void> {
    return this.call(RIGA_IPC_COMMANDS.approval, { request: { run_id: runId, approval_id: approvalId, approved, option } });
  }
  listMcpRegistry(): Promise<McpServerSummary[]> { return this.call(RIGA_IPC_COMMANDS.listMcpRegistry); }
  saveMcpRegistry(request: McpRegistryRequest): Promise<McpServerSummary[]> { return this.call(RIGA_IPC_COMMANDS.saveMcpRegistry, { request }); }

  async uploadAttachment(file: File, sessionId: string): Promise<Attachment> {
    const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
    return this.call(RIGA_IPC_COMMANDS.uploadAttachment, { attachment: { name: file.name, bytes, session_id: sessionId } });
  }

  listLocalModels(): Promise<import("../protocol").LocalModelOverview> {
    return this.call(RIGA_IPC_COMMANDS.localModels);
  }
  subscribeLocalModels(): () => void { return () => undefined; }
  downloadModel(modelId: string): Promise<void> { return this.localModelAction("download", { model_id: modelId }); }
  cancelDownload(modelId: string): Promise<void> { return this.localModelAction("cancel", { model_id: modelId }); }
  loadModel(path: string): Promise<void> { return this.localModelAction("load", { path }); }
  unloadModel(): Promise<void> { return this.localModelAction("unload"); }

  private async open(): Promise<void> {
    this.listeners.onStatus?.("connecting");
    try {
      this.unlisten = await Promise.all([
        this.listen<RigaEventEnvelope>(RIGA_IPC_EVENTS.run, (event) => this.listeners.onEvent?.(event.payload)),
        this.listen<import("../protocol").LocalModelEvent>(RIGA_IPC_EVENTS.localModel, (event) => this.listeners.onLocalModelEvent?.(event.payload)),
        this.listen<ProviderConfigured>(RIGA_IPC_EVENTS.providerConfigured, (event) => {
          const value = event.payload;
          this.listeners.onProviderConfigured?.(value.endpoint, value.model, value.reasoning_effort, value.kind ?? "remote", value.api ?? "chat", value.subagent_model ?? "");
        }),
        this.listen<TransportError>(RIGA_IPC_EVENTS.error, (event) => {
          this.listeners.onStatus?.("error");
          this.listeners.onError?.(event.payload.code, event.payload.message);
        }),
      ]);
      await this.invoke<void>(RIGA_IPC_COMMANDS.connect);
      this.connected = true;
      this.listeners.onStatus?.("connected");
    } catch (error) {
      for (const unsubscribe of this.unlisten.splice(0)) unsubscribe();
      this.listeners.onStatus?.("error");
      throw error;
    }
  }

  private async call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    await this.connect();
    return this.invoke<T>(command, args);
  }

  private localModelAction(action: string, extra: Record<string, unknown> = {}): Promise<void> {
    return this.call(RIGA_IPC_COMMANDS.localModelAction, { request: { action, ...extra } });
  }
}

export function createTauriTransport(options: TauriTransportOptions = {}): RigaTauriTransport {
  return new RigaTauriTransport(options);
}
