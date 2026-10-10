import { afterEach, describe, expect, it, vi } from "vitest";

// The HTTP transport builds a WebSocket client internally; the conformance suite
// replaces it so both transports can be driven without a socket.
const socketCalls: string[] = [];
vi.mock("../http/websocket", () => ({
  RigaWebSocketClient: class FakeSocket {
    constructor(_options: unknown) {}
    connect(): Promise<void> { socketCalls.push("connect"); return Promise.resolve(); }
    wake(): void { socketCalls.push("wake"); }
    close(): void { socketCalls.push("close"); }
    configureProvider(): Promise<void> { socketCalls.push("configureProvider"); return Promise.resolve(); }
    startRun(): Promise<void> { socketCalls.push("startRun"); return Promise.resolve(); }
    resumeRun(): Promise<void> { socketCalls.push("resumeRun"); return Promise.resolve(); }
    cancelRun(): Promise<void> { socketCalls.push("cancelRun"); return Promise.resolve(); }
    listActiveRuns(): Promise<[]> { socketCalls.push("listActiveRuns"); return Promise.resolve([]); }
    respondToApproval(): Promise<void> { socketCalls.push("respondToApproval"); return Promise.resolve(); }
  },
}));

import { RIGA_TRANSPORT_METHODS, type RigaTransport } from "./index";
import { createTauriTransport, type TauriTransportOptions } from "../tauri/index";
import { createHttpTransport } from "../http/index";

const invoke = (async <T>(_command: string, _args?: Record<string, unknown>) => undefined as T) as TauriTransportOptions["invoke"];
const listen = (async () => () => undefined) as TauriTransportOptions["listen"];

// Compile-time completeness. Every member of `RigaTransport` must appear in the
// runtime list; if one is added to the interface and not the list, `Missing` is
// not `never` and the assignment below fails to compile. This is the guard that
// keeps the IPC and WebSocket transports from drifting apart as the protocol
// grows.
type Missing = Exclude<keyof RigaTransport, (typeof RIGA_TRANSPORT_METHODS)[number]>;
const everyMemberIsListed: Missing extends never ? true : never = true;

// The request methods: everything that must reach the wire. The lifecycle
// methods (`connect`/`wake`/`close`/`subscribeLocalModels`) are excluded.
const REQUEST_METHODS = [
  "health",
  "catalog",
  "listSessions",
  "createSession",
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
  "downloadModel",
  "cancelDownload",
  "loadModel",
  "unloadModel",
] as const satisfies ReadonlyArray<keyof RigaTransport>;

type RequestMethod = (typeof REQUEST_METHODS)[number];

/** Call one request method with representative arguments. */
function callRequest(transport: RigaTransport, method: RequestMethod): Promise<unknown> {
  switch (method) {
    case "health": return transport.health();
    case "catalog": return transport.catalog();
    case "listSessions": return transport.listSessions();
    case "createSession": return transport.createSession({ title: "t", workspace: "/w" });
    case "configureProvider": return transport.configureProvider("https://model.test/v1", "key", "model", "low");
    case "startRun": return transport.startRun("run", "session", "hello");
    case "resumeRun": return transport.resumeRun("run", 1);
    case "cancelRun": return transport.cancelRun("run");
    case "listActiveRuns": return transport.listActiveRuns();
    case "respondToApproval": return transport.respondToApproval("run", "approval", true);
    case "listMcpRegistry": return transport.listMcpRegistry();
    case "saveMcpRegistry": return transport.saveMcpRegistry({ name: "health", transport: "stdio", command: "health-mcp" });
    case "uploadAttachment": return transport.uploadAttachment(new File(["hello"], "note.txt"), "session");
    case "listLocalModels": return transport.listLocalModels();
    case "downloadModel": return transport.downloadModel("model");
    case "cancelDownload": return transport.cancelDownload("model");
    case "loadModel": return transport.loadModel("/tmp/model.gguf");
    case "unloadModel": return transport.unloadModel();
  }
}

afterEach(() => {
  vi.restoreAllMocks();
  socketCalls.length = 0;
});

describe("RigaTransport conformance", () => {
  it("the runtime method list covers the whole interface", () => {
    expect(everyMemberIsListed).toBe(true);
    expect(new Set(RIGA_TRANSPORT_METHODS).size).toBe(RIGA_TRANSPORT_METHODS.length);
  });

  it("both transports implement every protocol member", () => {
    const transports: RigaTransport[] = [
      createTauriTransport({ invoke, listen }),
      createHttpTransport("https://riga.test"),
    ];
    for (const transport of transports) {
      for (const method of RIGA_TRANSPORT_METHODS) {
        expect(typeof transport[method], method).toBe("function");
      }
    }
  });

  it("both transports return a local-model unsubscribe function", () => {
    const tauri = createTauriTransport({ invoke, listen });
    expect(tauri.subscribeLocalModels()).toBeTypeOf("function");
    expect(createHttpTransport("https://riga.test").subscribeLocalModels()).toBeTypeOf("function");
  });

  // A transport must not stub a request method with a local value: every request
  // method has to reach the wire. This is what catches a fork like an IPC
  // `listActiveRuns` that returned `[]` without asking the server.
  it("the IPC transport routes every request method to a command", async () => {
    const commands: string[] = [];
    const transport = createTauriTransport({
      invoke: (async <T>(command: string) => { commands.push(command); return undefined as T; }) as TauriTransportOptions["invoke"],
      listen,
    });
    for (const method of REQUEST_METHODS) {
      commands.length = 0;
      await callRequest(transport, method);
      expect(commands, method).not.toHaveLength(0);
    }
  });

  it("the WebSocket transport routes every request method to the wire", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async () => new Response(JSON.stringify([]), { status: 200 }));
    const transport = createHttpTransport("https://riga.test");
    for (const method of REQUEST_METHODS) {
      socketCalls.length = 0;
      fetchMock.mockClear();
      await callRequest(transport, method);
      expect(socketCalls.length + fetchMock.mock.calls.length, method).toBeGreaterThan(0);
    }
  });
});
