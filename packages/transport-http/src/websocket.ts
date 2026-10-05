import type { RigaEventEnvelope } from "./index";

export type ProviderKind = "remote" | "local";

/** How long to wait for the server's `ready` frame before giving up and retrying. */
const HANDSHAKE_TIMEOUT_MS = 8_000;

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
  /** Set for the duration of a run so a reconnect can resume it. */
  private activeRunId: string | null = null;
  private lastSequence = 0;
  private reconnectAttempts = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private closedByUser = false;

  connect(): Promise<void> {
    if (this.ready) return this.ready;
    this.closedByUser = false;
    this.cancelReconnect();
    this.ready = this.openSocket();
    return this.ready;
  }

  /**
   * Reconnect now if the connection is down, without waiting out a backoff timer.
   *
   * Call this when the page resumes: a phone that was locked, a tab that was
   * backgrounded, or a network that just came back. While suspended, timers do
   * not fire, so the scheduled reconnect can be arbitrarily far in the future;
   * this restarts it immediately. A no-op when already connected or connecting.
   */
  wake(): void {
    if (this.closedByUser || this.ready) return;
    this.cancelReconnect();
    this.ready = this.openSocket();
    void this.ready.catch(() => undefined);
  }

  private cancelReconnect(): void {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private openSocket(): Promise<void> {
    this.onStatus("connecting");
    return new Promise<void>((resolve, reject) => {
      let socket: RigaWebSocketLike;
      try {
        socket = this.makeSocket(this.url);
      } catch (error) {
        // A malformed URL (e.g. a missing host) must not throw out of `connect`
        // and leave the client with no scheduled retry.
        this.onStatus("error");
        reject(error instanceof Error ? error : new Error("RIGA WebSocket could not be created"));
        if (!this.closedByUser) this.scheduleReconnect();
        return;
      }
      this.socket = socket;
      let handshakeTimer: ReturnType<typeof setTimeout> | null = null;
      const clearHandshake = () => {
        if (handshakeTimer !== null) {
          clearTimeout(handshakeTimer);
          handshakeTimer = null;
        }
      };
      // A socket can open and then never receive `ready` — a kernel still
      // building, or a proxy that accepted the upgrade but never delivered the
      // frame. Without this the status sits at "connecting" forever, which is
      // exactly what a phone showed after a restart.
      handshakeTimer = setTimeout(() => {
        clearHandshake();
        if (this.closedByUser) return;
        try {
          socket.close();
        } catch {
          // The socket may already be gone; the reconnect below still runs.
        }
        reject(new Error("RIGA WebSocket handshake timed out"));
        if (!this.closedByUser) this.scheduleReconnect();
      }, HANDSHAKE_TIMEOUT_MS);
      socket.onopen = () => {
        // A socket that was already replaced (a fast reconnect) must not send
        // its stale handshake into the new connection's state.
        if (socket !== this.socket) return;
        this.send({ type: "hello", client_version: "0.1.0" });
      };
      socket.onmessage = (message) => {
        if (socket !== this.socket) return;
        let parsed: RigaWebSocketServerMessage;
        try {
          parsed = JSON.parse(message.data) as RigaWebSocketServerMessage;
        } catch {
          return;
        }
        if (parsed.type === "ready") {
          clearHandshake();
          this.onStatus("connected");
          this.reconnectAttempts = 0;
          resolve();
          // A run that was interrupted by the drop is caught up from the
          // server's journal without re-running the agent.
          if (this.activeRunId) {
            void this.resumeRun(this.activeRunId, this.lastSequence).catch(() => undefined);
          }
        } else if (parsed.type === "event") {
          this.trackRun(parsed.envelope);
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
        if (socket !== this.socket) return;
        clearHandshake();
        this.onStatus("error");
        reject(new Error("RIGA WebSocket connection failed"));
        // Retry here as well as on close: some browsers fire `error` without a
        // following `close`, which would otherwise leave the client dead.
        if (!this.closedByUser) this.scheduleReconnect();
      };
      socket.onclose = () => {
        clearHandshake();
        // A superseded socket's close must not flap the status or schedule a
        // second reconnect on top of the one already in flight.
        if (socket !== this.socket) return;
        this.socket = null;
        this.onStatus("closed");
        // An unexpected drop reconnects with backoff; an explicit close does not.
        if (!this.closedByUser) this.scheduleReconnect();
      };
    });
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) return;
    this.ready = null;
    const delay = Math.min(1_000 * 2 ** this.reconnectAttempts, 10_000);
    this.reconnectAttempts += 1;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (this.closedByUser) return;
      this.ready = this.openSocket();
      void this.ready.catch(() => undefined);
    }, delay);
  }

  /** Track the active run and its last sequence, so a reconnect can resume it. */
  private trackRun(envelope: RigaEventEnvelope): void {
    this.lastSequence = Math.max(this.lastSequence, envelope.sequence);
    const event: unknown = envelope.event;
    const settled = event === "RunCompleted"
      || event === "RunFailed"
      || (typeof event === "object" && event !== null && ("RunCompleted" in event || "RunFailed" in event));
    if (settled && this.activeRunId === envelope.run_id) {
      this.activeRunId = null;
      this.lastSequence = 0;
    }
  }

  async startRun(runId: string, sessionId: string, prompt: string): Promise<void> {
    await this.connect();
    this.activeRunId = runId;
    this.lastSequence = 0;
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
    this.closedByUser = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.activeRunId = null;
    this.socket?.close();
    this.socket = null;
    this.ready = null;
  }

  private send(message: RigaWebSocketClientMessage): void {
    if (!this.socket) throw new Error("RIGA WebSocket is not connected");
    this.socket.send(JSON.stringify(message));
  }
}
