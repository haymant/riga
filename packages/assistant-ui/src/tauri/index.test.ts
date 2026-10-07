import { afterEach, describe, expect, it, vi } from "vitest";
import { RIGA_IPC_COMMANDS, RIGA_IPC_EVENTS, RigaTauriTransport, type TauriTransportOptions } from "./index";

type Handler = (event: { payload: unknown }) => void;

afterEach(() => vi.restoreAllMocks());

describe("RigaTauriTransport", () => {
  it("configures a provider and controls runs through IPC", async () => {
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
    const invoke = vi.fn(async <T>(command: string, args?: Record<string, unknown>) => {
      calls.push({ command, args });
      return undefined as T;
    });
    const listen = vi.fn(async () => () => undefined);
    const transport = new RigaTauriTransport({ invoke: invoke as TauriTransportOptions["invoke"], listen });

    await transport.configureProvider("https://model.test/v1", "secret", "model", "medium", "remote", "responses", "cheap");
    await transport.startRun("run-1", "session-1", "hello");
    await transport.resumeRun("run-1", 4);
    await transport.cancelRun("run-1");
    await transport.respondToApproval("run-1", "approval-1", true, "once");

    expect(calls.map(({ command }) => command)).toEqual([
      RIGA_IPC_COMMANDS.connect,
      RIGA_IPC_COMMANDS.configureProvider,
      RIGA_IPC_COMMANDS.startRun,
      RIGA_IPC_COMMANDS.resumeRun,
      RIGA_IPC_COMMANDS.cancelRun,
      RIGA_IPC_COMMANDS.approval,
    ]);
    expect(calls[1]?.args).toMatchObject({ endpoint: "https://model.test/v1", api_key: "secret", reasoning_effort: "medium", api: "responses" });
    expect(calls[4]?.args).toEqual({ run_id: "run-1" });
  });

  it("delivers run, local-model, provider, and error events", async () => {
    const handlers = new Map<string, Handler>();
    const events: unknown[] = [];
    const statuses: string[] = [];
    const transport = new RigaTauriTransport({
      invoke: (async <T>() => undefined as T) as TauriTransportOptions["invoke"],
      listen: async (name, handler) => { handlers.set(name, handler as Handler); return () => handlers.delete(name); },
      listeners: {
        onEvent: (event) => events.push(event),
        onLocalModelEvent: (event) => events.push(event),
        onProviderConfigured: (...value) => events.push(value),
        onError: (...value) => events.push(value),
        onStatus: (status) => statuses.push(status),
      },
    });

    await transport.connect();
    const envelope = { protocol_version: 1, event_id: "event-1", session_id: "s", run_id: "r", sequence: 1, timestamp: "now", event: "RunStarted" };
    handlers.get(RIGA_IPC_EVENTS.run)?.({ payload: envelope });
    handlers.get(RIGA_IPC_EVENTS.localModel)?.({ payload: { type: "download_finished", model_id: "m", path: "/tmp/m.gguf" } });
    handlers.get(RIGA_IPC_EVENTS.providerConfigured)?.({ payload: { endpoint: "e", model: "m", reasoning_effort: "low" } });
    handlers.get(RIGA_IPC_EVENTS.error)?.({ payload: { code: "bad", message: "broken" } });

    expect(events).toHaveLength(4);
    expect(events[0]).toEqual(envelope);
    expect(statuses).toEqual(["connecting", "connected", "error"]);
    transport.close();
    expect(handlers).toHaveLength(0);
  });

  it("passes the local-model action and attachment bytes to IPC", async () => {
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
    const transport = new RigaTauriTransport({
      invoke: async <T>(command: string, args?: Record<string, unknown>) => { calls.push({ command, args }); return undefined as T; },
      listen: async () => () => undefined,
    });
    await transport.health();
    await transport.catalog();
    await transport.listSessions();
    await transport.createSession({ title: "Test", workspace: "/tmp" });
    await transport.listMcpRegistry();
    await transport.saveMcpRegistry({ name: "health", transport: "stdio", command: "health-mcp" });
    await transport.cancelDownload("model-1");
    await transport.loadModel("/tmp/model.gguf");
    await transport.unloadModel();
    await transport.downloadModel("model-1");
    const file = new File([new Uint8Array([1, 2, 3])], "note.txt");
    await transport.uploadAttachment(file);
    expect(calls.find((call) => call.command === RIGA_IPC_COMMANDS.localModelAction && call.args?.action === "download")?.args).toEqual({ action: "download", model_id: "model-1" });
    expect(calls.find((call) => call.command === RIGA_IPC_COMMANDS.uploadAttachment)?.args).toEqual({ name: "note.txt", bytes: [1, 2, 3] });
  });
});
