import { describe, expect, it } from "vitest";
import { consumeEvents, defaultFormAnswer, type OpencodeHttp, type V2EventEnvelope } from "./events.js";
import type { OpenCodeApprovalRequest, OpenCodeStreamEvent } from "./protocol.js";

async function* streamOf(events: V2EventEnvelope[]): AsyncIterable<V2EventEnvelope> {
  for (const event of events) yield event;
}

function fakeIo() {
  const requests: Array<{ method: string; path: string; body?: unknown }> = [];
  const io: OpencodeHttp = {
    async request(method, path, body) {
      requests.push({ method, path, body });
    },
  };
  return { io, requests };
}

async function run(
  events: V2EventEnvelope[],
  approve?: (request: OpenCodeApprovalRequest) => Promise<"once" | "always" | "reject">,
  sessionID = "session-1",
) {
  const emitted: OpenCodeStreamEvent[] = [];
  const { io, requests } = fakeIo();
  const approvals: OpenCodeApprovalRequest[] = [];
  await consumeEvents(
    io,
    { stream: streamOf(events) },
    sessionID,
    (event) => { emitted.push(event); },
    approve ? async (request) => { approvals.push(request); return approve(request); } : undefined,
    new AbortController().signal,
  );
  return { emitted, requests, approvals };
}

describe("OpenCode event translation", () => {
  it("maps text, reasoning, and the tool lifecycle, and ignores other sessions", async () => {
    const { emitted } = await run([
      { type: "session.created", data: { sessionID: "session-1" } },
      { type: "session.tool.input.started", data: { sessionID: "session-1", id: "c1", name: "shell" } },
      { type: "session.tool.called", data: { sessionID: "session-1", id: "c1", input: { command: "ls" } } },
      { type: "session.tool.success", data: { sessionID: "session-1", id: "c1", content: [{ type: "text", text: "ok" }] } },
      { type: "session.text.delta", data: { sessionID: "session-1", delta: "hi" } },
      { type: "session.reasoning.delta", data: { sessionID: "session-1", delta: "think" } },
      { type: "session.text.delta", data: { sessionID: "session-2", delta: "other" } },
    ]);
    expect(emitted).toEqual([
      { type: "tool_started", callId: "c1", tool: "shell", input: { command: "ls" } },
      { type: "tool_finished", callId: "c1", tool: "shell", ok: true, output: "ok" },
      { type: "text", delta: "hi" },
      { type: "reasoning", delta: "think" },
    ]);
  });

  it("still accepts the SDK's session.next.* event names", async () => {
    const { emitted } = await run([
      { type: "session.next.tool.called", data: { sessionID: "session-1", callID: "c9", tool: "write", input: { path: "a" } } },
      { type: "session.next.tool.failed", data: { sessionID: "session-1", callID: "c9", error: { message: "denied" } } },
      { type: "session.next.text.delta", data: { sessionID: "session-1", delta: "x" } },
    ]);
    expect(emitted).toEqual([
      { type: "tool_started", callId: "c9", tool: "write", input: { path: "a" } },
      { type: "tool_finished", callId: "c9", tool: "write", ok: false, output: "denied" },
      { type: "text", delta: "x" },
    ]);
  });

  it("forwards a permission prompt and replies with the decision", async () => {
    const { approvals, requests } = await run(
      [{ type: "permission.asked", data: { sessionID: "session-1", id: "req-1", action: "shell", resources: ["rm -rf build"] } }],
      async () => "once",
    );
    expect(approvals).toEqual([{ kind: "permission", requestId: "req-1", action: "shell", resources: ["rm -rf build"] }]);
    expect(requests).toEqual([
      { method: "POST", path: "/api/session/session-1/permission/req-1/reply", body: { decision: "once" } },
    ]);
  });

  it("forwards a form and answers it with field defaults", async () => {
    const form = {
      id: "form-1",
      sessionID: "session-1",
      title: "Pick a colour",
      fields: {
        colour: { type: "string", options: [{ value: "red", label: "Red" }, { value: "blue", label: "Blue" }] },
        extras: { type: "multiselect", options: [{ value: "a", label: "A" }, { value: "b", label: "B" }] },
        enabled: { type: "boolean" },
        note: { type: "string" },
      },
    };
    const { approvals, requests } = await run(
      [{ type: "form.created", data: { form } }],
      async () => "once",
    );
    expect(approvals).toEqual([{ kind: "form", requestId: "form-1", form }]);
    expect(requests).toEqual([
      { method: "POST", path: "/api/session/session-1/form/form-1/reply", body: { answer: { colour: "red", extras: ["a"], enabled: true } } },
    ]);
    expect(defaultFormAnswer(form as unknown as Record<string, unknown>)).toEqual({ colour: "red", extras: ["a"], enabled: true });
  });

  it("cancels a form when the client declines", async () => {
    const { requests } = await run(
      [{ type: "form.created", data: { form: { id: "form-2", sessionID: "session-1" } } }],
      async () => "reject",
    );
    expect(requests).toEqual([{ method: "DELETE", path: "/api/session/session-1/form/form-2", body: undefined }]);
  });
});
