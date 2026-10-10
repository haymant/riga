export const RIGA_PROTOCOL_VERSION = 1 as const;

export type ProviderKind = "remote" | "local";
export type ProviderApi = "chat" | "responses";
export type TransportStatus = "connecting" | "connected" | "closed" | "error";

export type RigaEventEnvelope = {
  protocol_version: number;
  event_id: string;
  session_id: string;
  run_id: string;
  sequence: number;
  timestamp: string;
  event: Record<string, unknown> | string;
};

export type Health = {
  protocol_version: number;
  adapter: string;
  persistence_backend?: string;
};

export type CreateSessionRequest = { title: string; workspace: string };
export type Session = CreateSessionRequest & {
  id: string;
  created_at: string;
  updated_at: string;
};
export type ConversationTurn = { role: string; content: string };

export type CatalogItem = {
  id: string;
  kind: string;
  description: string;
  insert_text: string;
  requires_approval: boolean;
};
export type SkillSummary = { name: string; description: string; path: string };
export type AgentSummary = { name: string; aliases: string[]; purpose: string; read_only: boolean };
export type FileCandidate = { name: string; path: string };
export type McpServerSummary = {
  name: string;
  command: string;
  args?: string[];
  tools: string[];
  transport?: "stdio" | "http";
  url?: string;
  api_key_configured?: boolean;
};
export type Catalog = {
  tools?: CatalogItem[];
  skills?: SkillSummary[];
  agents?: AgentSummary[];
  files?: FileCandidate[];
  mcp_servers?: McpServerSummary[];
  [key: string]: unknown;
};

export type ProviderConfiguration = {
  endpoint: string;
  api_key: string;
  model: string;
  reasoning_effort: "low" | "medium" | "high";
  kind?: ProviderKind;
  api?: ProviderApi;
  subagent_model?: string;
};

export type Attachment = { name: string; path: string; size: number };
export type McpRegistryRequest = {
  name: string;
  transport: "stdio" | "http";
  command?: string;
  args?: string[];
  url?: string;
  api_key?: string;
};

export type CuratedModel = {
  id: string;
  name: string;
  file_name: string;
  size_bytes: number;
  sha256: string;
  recommended_context: number;
  max_context: number;
  quant: string;
  license_url: string;
};
export type InstalledModel = {
  id: string;
  name: string;
  file_name: string;
  path: string;
  size_bytes: number;
  curated: boolean;
  recommended_context: number | null;
  license_url: string | null;
};
export type LocalModelOverview = {
  accelerator: string;
  catalog: CuratedModel[];
  installed: InstalledModel[];
  loaded: string | null;
};
export type LocalModelEvent =
  | { type: "download_progress"; model_id: string; downloaded_bytes: number; total_bytes: number; percent: number }
  | { type: "download_finished"; model_id: string; path: string }
  | { type: "download_failed"; model_id: string; message: string }
  | { type: "token"; generation_id: string; delta: string };
export type DownloadState = {
  phase: "downloading" | "finished" | "failed";
  percent: number;
  downloaded_bytes: number;
  total_bytes: number;
  message?: string;
};

export type RigaTransportListeners = {
  onEvent?: (event: RigaEventEnvelope) => void;
  onError?: (code: string, message: string) => void;
  onStatus?: (status: TransportStatus) => void;
  onProviderConfigured?: (
    endpoint: string,
    model: string,
    reasoningEffort: "low" | "medium" | "high",
    kind: ProviderKind,
    api: ProviderApi,
    subagentModel: string,
  ) => void;
  onLocalModelEvent?: (event: LocalModelEvent) => void;
};

export interface RigaTransport {
  connect(): Promise<void>;
  wake(): void;
  close(): void;
  health(): Promise<Health>;
  catalog(): Promise<Catalog>;
  listSessions(): Promise<Session[]>;
  createSession(request: CreateSessionRequest): Promise<Session>;
  sessionHistory(sessionId: string): Promise<ConversationTurn[]>;
  renameSession(sessionId: string, title: string): Promise<Session>;
  configureProvider(
    endpoint: string,
    apiKey: string,
    model: string,
    reasoningEffort: "low" | "medium" | "high",
    kind?: ProviderKind,
    api?: ProviderApi,
    subagentModel?: string,
  ): Promise<void>;
  startRun(runId: string, sessionId: string, prompt: string): Promise<void>;
  resumeRun(runId: string, afterSequence: number): Promise<void>;
  cancelRun(runId: string): Promise<void>;
  listActiveRuns(): Promise<ActiveRun[]>;
  respondToApproval(runId: string, approvalId: string, approved: boolean, option?: "once" | "always"): Promise<void>;
  listMcpRegistry(): Promise<McpServerSummary[]>;
  saveMcpRegistry(request: McpRegistryRequest): Promise<McpServerSummary[]>;
  uploadAttachment(file: File, sessionId: string): Promise<Attachment>;
  listLocalModels(): Promise<LocalModelOverview>;
  subscribeLocalModels(): () => void;
  downloadModel(modelId: string): Promise<void>;
  cancelDownload(modelId: string): Promise<void>;
  loadModel(path: string): Promise<void>;
  unloadModel(): Promise<void>;
}

export type ActiveRun = { run_id: string; session_id: string; local: boolean };

/**
 * Every member of {@link RigaTransport}, as a runtime list.
 *
 * A TypeScript interface is not enumerable, so a test cannot walk `RigaTransport`
 * directly. This list is the runtime stand-in, and `protocol/conformance.test.ts`
 * checks it for completeness at compile time: adding a method to the interface
 * without adding it here is a type error. That is what keeps the IPC and
 * WebSocket transports from drifting apart as the protocol grows.
 */
export const RIGA_TRANSPORT_METHODS = [
  "connect",
  "wake",
  "close",
  "health",
  "catalog",
  "listSessions",
  "createSession",
  "sessionHistory",
  "renameSession",
  "configureProvider",
  "startRun",
  "resumeRun",
  "cancelRun",
  "listActiveRuns",
  "respondToApproval",
  "listMcpRegistry",
  "saveMcpRegistry",
  "uploadAttachment",
  "listLocalModels",
  "subscribeLocalModels",
  "downloadModel",
  "cancelDownload",
  "loadModel",
  "unloadModel",
] as const satisfies ReadonlyArray<keyof RigaTransport>;
