import { describe, expect, it } from "vitest";
import { RIGA_TRANSPORT_METHODS, type RigaTransport } from "./index";
import { createTauriTransport, type TauriTransportOptions } from "../tauri/index";
import { createHttpTransport } from "../http/index";

// The IPC transport's `invoke` is generic over the return type; a no-op fake must
// be too, or it is not assignable to the `Invoke` signature.
const invoke = (async <T>(_command: string, _args?: Record<string, unknown>) => undefined as T) as TauriTransportOptions["invoke"];
const listen = (async () => () => undefined) as TauriTransportOptions["listen"];

// Compile-time completeness. Every member of `RigaTransport` must appear in the
// runtime list; if one is added to the interface and not the list, `Missing` is
// not `never` and the assignment below fails to compile. This is the guard that
// keeps the IPC and WebSocket transports from drifting apart as the protocol
// grows.
type Missing = Exclude<keyof RigaTransport, (typeof RIGA_TRANSPORT_METHODS)[number]>;
const everyMemberIsListed: Missing extends never ? true : never = true;

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
});
