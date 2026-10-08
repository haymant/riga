import { afterEach, describe, expect, it, vi } from "vitest";

const socketCalls: string[] = [];

vi.mock("./websocket", () => ({
  RigaWebSocketClient: class FakeWebSocketClient {
    constructor(_options: unknown) {}
    connect(): Promise<void> { socketCalls.push("connect"); return Promise.resolve(); }
    wake(): void { socketCalls.push("wake"); }
    close(): void { socketCalls.push("close"); }
    configureProvider(..._args: unknown[]): Promise<void> { socketCalls.push("configureProvider"); return Promise.resolve(); }
    startRun(..._args: unknown[]): Promise<void> { socketCalls.push("startRun"); return Promise.resolve(); }
    resumeRun(..._args: unknown[]): Promise<void> { socketCalls.push("resumeRun"); return Promise.resolve(); }
    cancelRun(..._args: unknown[]): Promise<void> { socketCalls.push("cancelRun"); return Promise.resolve(); }
    respondToApproval(..._args: unknown[]): Promise<void> { socketCalls.push("respondToApproval"); return Promise.resolve(); }
  },
}));

import { createHttpTransport, RigaHttpClient, RigaHttpTransport } from "./index";

afterEach(() => {
  vi.restoreAllMocks();
  socketCalls.length = 0;
});

describe("RigaHttpClient", () => {
  it("requests health and session resources with the expected HTTP contract", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = String(input);
      if (url.endsWith("/health")) return new Response(JSON.stringify({ protocol_version: 1, adapter: "riga-server" }), { status: 200 });
      if (url.endsWith("/sessions") && init?.method === "POST") return new Response(JSON.stringify({ id: "session-1", title: "Test", workspace: "/tmp", created_at: "now", updated_at: "now" }), { status: 201 });
      return new Response(JSON.stringify([]), { status: 200 });
    });
    const client = new RigaHttpClient("http://riga.test");
    await expect(client.health()).resolves.toEqual({ protocol_version: 1, adapter: "riga-server" });
    await expect(client.listSessions()).resolves.toEqual([]);
    await expect(client.createSession({ title: "Test", workspace: "/tmp" })).resolves.toMatchObject({ id: "session-1" });
    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(fetchMock.mock.calls[2][1]).toMatchObject({ method: "POST" });
  });

  it("surfaces non-2xx HTTP responses", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("nope", { status: 503 }));
    await expect(new RigaHttpClient("http://riga.test").health()).rejects.toThrow("RIGA request failed with 503");
  });

  it("parses split SSE frames and URL-encodes run identifiers", async () => {
    const event = { protocol_version: 1, event_id: "e-1", session_id: "s-1", run_id: "run/a", sequence: 1, timestamp: "now", event: "RunStarted" };
    const chunks = ["event: riga.event\ndata: ", JSON.stringify(event).slice(0, 31), `${JSON.stringify(event).slice(31)}\n\n`];
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(new TextEncoder().encode(chunk));
        controller.close();
      },
    });
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(stream, { status: 200 }));
    const received: unknown[] = [];
    await new RigaHttpClient("http://riga.test").streamRunEvents("run/a", (value) => received.push(value));
    expect(received).toEqual([event]);
    expect(String(fetchMock.mock.calls[0][0])).toBe("http://riga.test/runs/run%2Fa/events");
  });

  it("rejects a failed or body-less SSE response", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("nope", { status: 500 }));
    await expect(new RigaHttpClient("http://riga.test").streamRunEvents("run", vi.fn())).rejects.toThrow("SSE request failed with 500");
    vi.restoreAllMocks();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 200 }));
    await expect(new RigaHttpClient("http://riga.test").streamRunEvents("run", vi.fn())).rejects.toThrow("SSE request failed with 200");
  });
});

describe("RigaHttpTransport", () => {
  it("delegates resources, runs, MCP, attachments, and local models", async () => {
    vi.stubGlobal("EventSource", class {
      addEventListener(): void {}
      close(): void {}
    });
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (_input, init) => {
      if (init?.method === "POST") return new Response(JSON.stringify({ ok: true }), { status: 200 });
      return new Response(JSON.stringify({ ok: true }), { status: 200 });
    });
    const localEvents: unknown[] = [];
    const transport = new RigaHttpTransport("https://riga.test", { onLocalModelEvent: (event) => localEvents.push(event) });
    await transport.connect();
    transport.wake();
    await transport.health();
    await transport.catalog();
    await transport.listSessions();
    await transport.createSession({ title: "Test", workspace: "/tmp" });
    await transport.configureProvider("https://model.test/v1", "key", "model", "low");
    await transport.startRun("run", "session", "hello");
    await transport.resumeRun("run", 1);
    await transport.cancelRun("run");
    await transport.respondToApproval("run", "approval", true, "once");
    await transport.listMcpRegistry();
    await transport.saveMcpRegistry({ name: "health", transport: "stdio", command: "health-mcp" });
    await transport.uploadAttachment(new File(["hello"], "note.txt"), "session");
    await transport.listLocalModels();
    await transport.downloadModel("model");
    await transport.cancelDownload("model");
    await transport.loadModel("/tmp/model.gguf");
    await transport.unloadModel();
    expect(fetchMock.mock.calls.some(([input]) => String(input).endsWith("/catalog"))).toBe(true);
    expect(fetchMock.mock.calls.some(([input]) => String(input).includes("/attachments?session=session"))).toBe(true);
    expect(socketCalls).toEqual(["connect", "wake", "configureProvider", "startRun", "resumeRun", "cancelRun", "respondToApproval"]);
    expect(localEvents).toEqual([]);
    transport.close();
    expect(socketCalls.at(-1)).toBe("close");
  });

  it("returns the local-model subscription cleanup and factory transport", () => {
    const transport = createHttpTransport("https://riga.test");
    expect(transport).toBeInstanceOf(RigaHttpTransport);
    expect(transport.subscribeLocalModels()).toBeTypeOf("function");
  });
});
