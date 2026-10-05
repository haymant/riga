import { describe, expect, it, vi } from "vitest";
import { RigaWebSocketClient, type RigaWebSocketLike } from "./websocket";

class FakeSocket implements RigaWebSocketLike {
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  sent: string[] = [];
  closed = false;
  send(data: string) { this.sent.push(data); }
  close() { this.closed = true; this.onclose?.(); }
  open() { this.onopen?.(); }
  receive(message: unknown) { this.onmessage?.({ data: JSON.stringify(message) }); }
  fail() { this.onerror?.(); }
}

const envelope = {
  protocol_version: 1,
  event_id: "run-1-1",
  session_id: "session-1",
  run_id: "run-1",
  sequence: 1,
  timestamp: "now",
  event: "RunStarted",
};

describe("RigaWebSocketClient", () => {
  it("performs a hello handshake and resolves only after ready", async () => {
    const socket = new FakeSocket();
    const statuses: string[] = [];
    const client = new RigaWebSocketClient({ url: "ws://test/ws", socketFactory: () => socket, onEvent: vi.fn(), onStatus: (status) => statuses.push(status) });
    const ready = client.connect();
    expect(statuses).toEqual(["connecting"]);
    socket.open();
    expect(JSON.parse(socket.sent[0])).toEqual({ type: "hello", client_version: "0.1.0" });
    let resolved = false;
    void ready.then(() => { resolved = true; });
    await Promise.resolve();
    expect(resolved).toBe(false);
    socket.receive({ type: "ready", protocol_version: 1, server_version: "0.1.0" });
    await ready;
    expect(statuses).toEqual(["connecting", "connected"]);
  });

  it("sends start, approval, cancel, and ping-compatible command frames", async () => {
    const socket = new FakeSocket();
    const client = new RigaWebSocketClient({ url: "ws://test/ws", socketFactory: () => socket, onEvent: vi.fn() });
    const connecting = client.connect();
    socket.open();
    socket.receive({ type: "ready", protocol_version: 1, server_version: "0.1.0" });
    await connecting;
    await client.startRun("run-1", "session-1", "fix tests");
    await client.respondToApproval("run-1", "approval-1", true);
    await client.cancelRun("run-1");
    expect(socket.sent.slice(1).map((value) => JSON.parse(value))).toEqual([
      { type: "start_run", run_id: "run-1", session_id: "session-1", prompt: "fix tests" },
      { type: "approval", run_id: "run-1", approval_id: "approval-1", approved: true },
      { type: "cancel_run", run_id: "run-1" },
    ]);
  });

  it("sends provider endpoint, key, and model only in the live socket frame", async () => {
    const socket = new FakeSocket();
    const client = new RigaWebSocketClient({ url: "ws://test/ws", socketFactory: () => socket, onEvent: vi.fn() });
    const ready = client.connect();
    socket.open();
    socket.receive({ type: "ready", protocol_version: 1, server_version: "0.1.0" });
    await ready;
    await client.configureProvider("https://api.example/v1", "ephemeral-key", "opencode-go", "low");
    expect(JSON.parse(socket.sent.at(-1)!)).toEqual({ type: "configure_provider", endpoint: "https://api.example/v1", api_key: "ephemeral-key", model: "opencode-go", reasoning_effort: "low", kind: "remote" });
  });

  it("sends kind local so the server runs the in-process GGUF", async () => {
    // The endpoint and key are meaningless for a local run, but sending the same
    // shape keeps one configure path in the UI.
    const socket = new FakeSocket();
    const client = new RigaWebSocketClient({ url: "ws://test/ws", socketFactory: () => socket, onEvent: vi.fn() });
    const ready = client.connect();
    socket.open();
    socket.receive({ type: "ready", protocol_version: 1, server_version: "0.1.0" });
    await ready;
    await client.configureProvider("", "", "local", "low", "local");
    const frame = JSON.parse(socket.sent.at(-1)!) as Record<string, unknown>;
    expect(frame.kind).toBe("local");
    expect(frame.type).toBe("configure_provider");
  });

  it("delivers ordered kernel event envelopes without rewriting them", async () => {
    const socket = new FakeSocket();
    const events: unknown[] = [];
    const client = new RigaWebSocketClient({ url: "ws://test/ws", socketFactory: () => socket, onEvent: (event) => events.push(event) });
    const ready = client.connect();
    socket.open();
    socket.receive({ type: "ready", protocol_version: 1, server_version: "0.1.0" });
    await ready;
    socket.receive({ type: "event", envelope });
    expect(events).toEqual([envelope]);
  });

  it("marks protocol errors and rejects connection failures", async () => {
    const socket = new FakeSocket();
    const statuses: string[] = [];
    const client = new RigaWebSocketClient({ url: "ws://test/ws", socketFactory: () => socket, onEvent: vi.fn(), onStatus: (status) => statuses.push(status) });
    const ready = client.connect();
    socket.open();
    socket.receive({ type: "error", code: "invalid_message", message: "bad frame" });
    expect(statuses).toContain("error");
    socket.fail();
    await expect(ready).rejects.toThrow("connection failed");
  });

  it("can close and reconnect with a fresh socket", async () => {
    const sockets: FakeSocket[] = [];
    const client = new RigaWebSocketClient({ url: "ws://test/ws", socketFactory: () => { const socket = new FakeSocket(); sockets.push(socket); return socket; }, onEvent: vi.fn() });
    const first = client.connect();
    sockets[0]?.open();
    sockets[0]?.receive({ type: "ready", protocol_version: 1, server_version: "0.1.0" });
    await first;
    client.close();
    expect(sockets[0].closed).toBe(true);
    const second = client.connect();
    sockets[1]?.open();
    sockets[1]?.receive({ type: "ready", protocol_version: 1, server_version: "0.1.0" });
    await second;
    expect(sockets).toHaveLength(2);
  });
});

it("restores provider metadata without receiving a credential", async () => {
  const socket = new FakeSocket();
  const restored: string[] = [];
  const client = new RigaWebSocketClient({ url: "ws://test/ws", socketFactory: () => socket, onEvent: vi.fn(), onProviderConfigured: (endpoint, model) => restored.push(`${endpoint}|${model}`) });
  const ready = client.connect();
  socket.open();
  socket.receive({ type: "ready", protocol_version: 1, server_version: "0.1.0" });
  await ready;
  socket.receive({ type: "provider_configured", endpoint: "https://api.example/v1", model: "opencode-go", reasoning_effort: "low" });
  expect(restored).toEqual(["https://api.example/v1|opencode-go"]);
});
