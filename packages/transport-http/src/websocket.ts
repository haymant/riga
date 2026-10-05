import type { RigaEventEnvelope } from "./index";

export type ProviderKind = "remote" | "local";

/**
 * Which remote API shape to call. `chat` is the OpenAI-compatible
 * `/chat/completions` contract; `responses` is OpenAI's `/responses` API.
 */
export type ProviderApi = "chat" | "responses";

export type RigaWebSocketClientMessage =
  | { type: "hello"; client_version: string }
  | { type: "configure_provider"; endpoint: string; api_key: string; model: string; reasoning_effort: "low" | "medium" | "high"; kind?: ProviderKind; api?: ProviderApi; subagent_model?: string }
  | { type: "start_run"; run_id: string; session_id: string; prompt: string }
  | { type: "resume_run"; run_id: string; after_sequence: number }
  | { type: "cancel_run"; run_id: string }
  | { type: "approval"; run_id: string; approval_id: string; approved: boolean; option?: "once" | "always" }
  | { type: "ping"; nonce: string };

export type RigaWebSocketServerMessage =
  | { type: "ready"; protocol_version: number; server_version: string }
  | { type: "provider_configured"; endpoint: string; model: string; reasoning_effort: "low" | "medium" | "high"; kind?: ProviderKind; api?: ProviderApi; subagent_model?: string }
  | { type: "event"; envelope: RigaEventEnvelope }
  | { type: "run_cancelled"; run_id: string }
  | { type: "approval_recorded"; run_id: string; approval_id: string; approved: boolean }
  | { type: "pong"; nonce: string }
  | { type: "error"; code: string; message: string };

export interface RigaWebSocketLike {
  onopen: (() => void) | null;
  onmessage: ((event: { data: string }) => void) | null;
  onerror: (() => void) | null;
  onclose: (() => void) | null;
  send(data: string): void;
  close(): void;
}

export type RigaWebSocketFactory = (url: string) => RigaWebSocketLike;

export class RigaWebSocketClient {
  private socket: RigaWebSocketLike | null = null;
  private ready: Promise<void> | null = null;
  private readonly makeSocket: RigaWebSocketFactory;
  private readonly onEvent: (envelope: RigaEventEnvelope) => void;
  private readonly onError: (code: string, message: string) => void;
  private readonly onStatus: (status: "connecting" | "connected" | "closed" | "error") => void;
  private readonly onProviderConfigured: (endpoint: string, model: string, reasoningEffort: "low" | "medium" | "high", kind: ProviderKind, api: ProviderApi, subagentModel: string) => void;

  constructor(options: { url: string; onEvent: (envelope: RigaEventEnvelope) => void; onError?: (code: string, message: string) => void; onStatus?: (status: "connecting" | "connected" | "closed" | "error") => void; onProviderConfigured?: (endpoint: string, model: string, reasoningEffort: "low" | "medium" | "high", kind: ProviderKind, api: ProviderApi, subagentModel: string) => void; socketFactory?: RigaWebSocketFactory }) {
    this.url = options.url;
    this.onEvent = options.onEvent;
    this.onError = options.onError ?? (() => undefined);
    this.onStatus = options.onStatus ?? (() => undefined);
    this.onProviderConfigured = options.onProviderConfigured ?? (() => undefined);
    this.makeSocket = options.socketFactory ?? ((url) => {
      const native = new WebSocket(url);
      let onopen: (() => void) | null = null;
      let onmessage: ((event: { data: string }) => void) | null = null;
      let onerror: (() => void) | null = null;
      let onclose: (() => void) | null = null;
      native.onopen = () => onopen?.();
      native.onmessage = (event) => onmessage?.({ data: event.data });
      native.onerror = () => onerror?.();
      native.onclose = () => onclose?.();
      return {
        get onopen() { return onopen; },
        set onopen(value) { onopen = value; },
        get onmessage() { return onmessage; },
        set onmessage(value) { onmessage = value; },
        get onerror() { return onerror; },
        set onerror(value) { onerror = value; },
        get onclose() { return onclose; },
        set onclose(value) { onclose = value; },
        send: (data: string) => native.send(data),
        close: () => native.close(),
      } satisfies RigaWebSocketLike;
    });
  }

  private readonly url: string;

  connect(): Promise<void> {
    if (this.ready) return this.ready;
    this.onStatus("connecting");
    this.ready = new Promise<void>((resolve, reject) => {
      const socket = this.makeSocket(this.url);
      this.socket = socket;
      socket.onopen = () => {
        this.send({ type: "hello", client_version: "0.1.0" });
      };
      socket.onmessage = (message) => {
        const parsed = JSON.parse(message.data) as RigaWebSocketServerMessage;
        if (parsed.type === "ready") {
          this.onStatus("connected");
          resolve();
        } else if (parsed.type === "event") {
          this.onEvent(parsed.envelope);
        } else if (parsed.type === "provider_configured") {
          // Older servers omit `kind`/`api`; a client written before those
          // existed must still read the frame as remote over chat completions.
          this.onProviderConfigured(parsed.endpoint, parsed.model, parsed.reasoning_effort, parsed.kind ?? "remote", parsed.api ?? "chat", parsed.subagent_model ?? "");
        } else if (parsed.type === "error") {
          this.onStatus("error");
          this.onError(parsed.code, parsed.message);
        }
      };
      socket.onerror = () => {
        this.onStatus("error");
        reject(new Error("RIGA WebSocket connection failed"));
      };
      socket.onclose = () => this.onStatus("closed");
    });
    return this.ready;
  }

  async startRun(runId: string, sessionId: string, prompt: string): Promise<void> {
    await this.connect();
    this.send({ type: "start_run", run_id: runId, session_id: sessionId, prompt });
  }

  /**
   * Replay a run's events after a sequence cursor.
   *
   * A client that disconnected mid-run reconnects, then calls this with the last
   * sequence it saw to catch up without re-running the agent.
   */
  async resumeRun(runId: string, afterSequence: number): Promise<void> {
    await this.connect();
    this.send({ type: "resume_run", run_id: runId, after_sequence: afterSequence });
  }

  async configureProvider(endpoint: string, apiKey: string, model: string, reasoningEffort: "low" | "medium" | "high", kind: ProviderKind = "remote", api: ProviderApi = "chat", subagentModel = ""): Promise<void> {
    await this.connect();
    this.send({ type: "configure_provider", endpoint, api_key: apiKey, model, reasoning_effort: reasoningEffort, kind, api, subagent_model: subagentModel || undefined });
  }

  async cancelRun(runId: string): Promise<void> {
    await this.connect();
    this.send({ type: "cancel_run", run_id: runId });
  }

  async respondToApproval(runId: string, approvalId: string, approved: boolean, option?: "once" | "always"): Promise<void> {
    await this.connect();
    this.send({ type: "approval", run_id: runId, approval_id: approvalId, approved, option });
  }

  close(): void {
    this.socket?.close();
    this.socket = null;
    this.ready = null;
  }

  private send(message: RigaWebSocketClientMessage): void {
    if (!this.socket) throw new Error("RIGA WebSocket is not connected");
    this.socket.send(JSON.stringify(message));
  }
}
