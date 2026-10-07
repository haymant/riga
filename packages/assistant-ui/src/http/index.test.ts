import { afterEach, describe, expect, it, vi } from "vitest";
import { RigaHttpClient } from "./index";

afterEach(() => vi.restoreAllMocks());

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
