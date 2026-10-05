export const RIGA_HTTP_TRANSPORT_VERSION = "0.1.0" as const;

export type RigaEventEnvelope = {
  protocol_version: number;
  event_id: string;
  session_id: string;
  run_id: string;
  sequence: number;
  timestamp: string;
  event: Record<string, unknown>;
};

export type CreateSessionRequest = { title: string; workspace: string };
export type Session = CreateSessionRequest & { id: string; created_at: string; updated_at: string };

export class RigaHttpClient {
  constructor(private readonly baseUrl: string) {}

  async health(): Promise<{ protocol_version: number; adapter: string }> {
    return this.request("/health");
  }

  async listSessions(): Promise<Session[]> {
    return this.request("/sessions");
  }

  async createSession(request: CreateSessionRequest): Promise<Session> {
    return this.request("/sessions", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(request) });
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

export * from "./websocket";
