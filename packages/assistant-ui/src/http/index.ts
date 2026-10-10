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
} from "../protocol";
import { LocalModelClient, reduceDownloadState, formatBytes } from "./local-models";
import { RigaWebSocketClient } from "./websocket";

export const RIGA_HTTP_TRANSPORT_VERSION = "0.1.0" as const;

export class RigaHttpClient {
  constructor(private readonly baseUrl: string) {}
  async health(): Promise<Health> { return this.request("/health"); }
  async listSessions(): Promise<Session[]> { return this.request("/sessions"); }
  async createSession(request: CreateSessionRequest): Promise<Session> {
    return this.request("/sessions", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(request) });
  }
  async sessionHistory(sessionId: string): Promise<import("../protocol").ConversationTurn[]> {
    return this.request(`/sessions/${encodeURIComponent(sessionId)}/history`);
  }
  async renameSession(sessionId: string, title: string): Promise<Session> {
    return this.request(`/sessions/${encodeURIComponent(sessionId)}`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ title }) });
  }
  async streamRunEvents(runId: string, onEvent: (event: RigaEventEnvelope) => void): Promise<void> {
    const response = await fetch(`${this.baseUrl}/runs/${encodeURIComponent(runId)}/events`, { headers: { accept: "text/event-stream" } });
    if (!response.ok || !response.body) throw new Error(`SSE request failed with ${response.status}`);
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const frames = buffer.split("\n\n");
      buffer = frames.pop() ?? "";
      for (const frame of frames) {
        const data = frame.split("\n").find((line) => line.startsWith("data:"));
        if (data) onEvent(JSON.parse(data.slice(5).trim()) as RigaEventEnvelope);
      }
    }
  }
  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, init);
    if (!response.ok) throw new Error(`RIGA request failed with ${response.status}`);
    return (await response.json()) as T;
  }
}

/** Thin browser adapter: it only frames calls for riga-server. */
export class RigaHttpTransport implements RigaTransport {
  private readonly resources: RigaHttpClient;
  private readonly localModels: LocalModelClient;
  private readonly socket: RigaWebSocketClient;
  private readonly localModelUnsubscribe: () => void;
  constructor(private readonly baseUrl = "", listeners: RigaTransportListeners = {}) {
    this.resources = new RigaHttpClient(baseUrl);
    this.localModels = new LocalModelClient(baseUrl);
    this.localModelUnsubscribe = listeners.onLocalModelEvent
      ? this.localModels.subscribe(listeners.onLocalModelEvent)
      : () => undefined;
    this.socket = new RigaWebSocketClient({ url: resolveWebSocketUrl(baseUrl), ...listeners, onEvent: listeners.onEvent ?? (() => undefined) });
  }
  connect(): Promise<void> { return this.socket.connect(); }
  wake(): void { this.socket.wake(); }
  close(): void { this.localModelUnsubscribe(); this.socket.close(); }
  health(): Promise<Health> { return this.resources.health(); }
  catalog(): Promise<Catalog> { return this.request("/catalog"); }
  listSessions(): Promise<Session[]> { return this.resources.listSessions(); }
  createSession(request: CreateSessionRequest): Promise<Session> { return this.resources.createSession(request); }
  sessionHistory(sessionId: string): Promise<import("../protocol").ConversationTurn[]> { return this.resources.sessionHistory(sessionId); }
  renameSession(sessionId: string, title: string): Promise<Session> { return this.resources.renameSession(sessionId, title); }
  configureProvider(...args: Parameters<RigaWebSocketClient["configureProvider"]>): Promise<void> { return this.socket.configureProvider(...args); }
  startRun(...args: Parameters<RigaWebSocketClient["startRun"]>): Promise<void> { return this.socket.startRun(...args); }
  resumeRun(...args: Parameters<RigaWebSocketClient["resumeRun"]>): Promise<void> { return this.socket.resumeRun(...args); }
  cancelRun(...args: Parameters<RigaWebSocketClient["cancelRun"]>): Promise<void> { return this.socket.cancelRun(...args); }
  listActiveRuns(): Promise<ActiveRun[]> { return this.socket.listActiveRuns(); }
  respondToApproval(...args: Parameters<RigaWebSocketClient["respondToApproval"]>): Promise<void> { return this.socket.respondToApproval(...args); }
  listMcpRegistry(): Promise<McpServerSummary[]> { return this.request("/mcp/registry"); }
  saveMcpRegistry(request: McpRegistryRequest): Promise<McpServerSummary[]> {
    return this.request("/mcp/registry", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(request) });
  }
  async uploadAttachment(file: File, sessionId: string): Promise<Attachment> {
    const form = new FormData();
    form.append("file", file, file.name);
    // The server stores the file in the session's worktree, so it must know the
    // session; the agent's tools then read it at the returned relative path.
    return this.request(`/attachments?session=${encodeURIComponent(sessionId)}`, { method: "POST", body: form });
  }
  listLocalModels() { return this.localModels.overview(); }
  subscribeLocalModels(): () => void { return this.localModelUnsubscribe; }
  downloadModel(modelId: string): Promise<void> { return this.localModels.download(modelId); }
  cancelDownload(modelId: string): Promise<void> { return this.localModels.cancelDownload(modelId); }
  loadModel(path: string): Promise<void> { return this.localModels.load(path); }
  unloadModel(): Promise<void> { return this.localModels.unload(); }
  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, init);
    if (!response.ok) {
      const detail = await response.json().catch(() => undefined) as { error?: string } | undefined;
      throw new Error(detail?.error ?? `RIGA request failed with ${response.status}`);
    }
    return (await response.json()) as T;
  }
}

export { RigaWebSocketClient } from "./websocket";
export { LocalModelClient, formatBytes, reduceDownloadState } from "./local-models";
export type { DownloadState, LocalModelOverview, LocalModelEvent, RigaEventEnvelope } from "../protocol";

export function createHttpTransport(baseUrl = "", listeners: RigaTransportListeners = {}): RigaHttpTransport {
  return new RigaHttpTransport(baseUrl, listeners);
}

function resolveWebSocketUrl(baseUrl: string): string {
  const url = new URL(baseUrl || `${window.location.protocol}//${window.location.host}`);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = "/ws";
  return url.toString();
}
