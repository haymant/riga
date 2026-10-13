import type {
  OpenCodeApprovalReply,
  OpenCodeApprovalRequest,
  OpenCodeStreamEvent,
} from "./protocol.js";

/**
 * Minimal authenticated HTTP surface for OpenCode routes the SDK does not expose
 * with the shape the running server expects (permission and form replies).
 */
export type OpencodeHttp = {
  request(method: string, path: string, body?: unknown): Promise<void>;
};

export function createOpencodeHttp(baseUrl: string, authorization?: string): OpencodeHttp {
  return {
    async request(method, path, body) {
      const response = await fetch(new URL(path, baseUrl), {
        method,
        headers: {
          ...(authorization ? { authorization } : {}),
          ...(body !== undefined ? { "content-type": "application/json" } : {}),
        },
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      if (!response.ok) {
        throw new Error(`OpenCode ${method} ${path} failed with HTTP ${response.status}`);
      }
    },
  };
}

export type V2EventEnvelope = {
  type?: string;
  data?: Record<string, unknown>;
};

export type V2EventStream = { stream: AsyncIterable<V2EventEnvelope> };

/** Translate OpenCode's tool result content parts into display text. */
export function toolContentText(content: unknown): string {
  if (!Array.isArray(content)) return "";
  return content
    .flatMap((part) => {
      if (!part || typeof part !== "object") return [];
      const record = part as Record<string, unknown>;
      if (record.type === "text" && typeof record.text === "string") return [record.text];
      if (record.type === "file" && typeof record.path === "string") return [String(record.path)];
      return [];
    })
    .join("");
}

export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message) return message;
    try {
      return JSON.stringify(error);
    } catch {
      return String(error);
    }
  }
  return String(error ?? "");
}

function stringField(value: unknown, key: string): string | undefined {
  if (!value || typeof value !== "object") return undefined;
  const field = (value as Record<string, unknown>)[key];
  return typeof field === "string" ? field : undefined;
}

function eventData(envelope: V2EventEnvelope): Record<string, unknown> | undefined {
  return envelope.data && typeof envelope.data === "object" ? envelope.data : undefined;
}

/** First option label per question, for the legacy question tool. */
export function firstAnswers(questions: unknown): Array<Array<string>> {
  if (!Array.isArray(questions)) return [];
  return questions.map((question) => {
    const options = question && typeof question === "object"
      ? (question as { options?: unknown }).options
      : undefined;
    const first = Array.isArray(options)
      ? options.find((option): option is { label: string } =>
        Boolean(option) && typeof option === "object" && typeof (option as { label?: unknown }).label === "string")
      : undefined;
    return first ? [first.label] : [];
  });
}

/**
 * Best-effort answer for a form: prefer an explicit default, then the first
 * option, then `true` for a boolean. Fields without a usable default are left
 * out; if OpenCode rejects the answer the form is cancelled instead.
 */
export function defaultFormAnswer(form: Record<string, unknown> | undefined): Record<string, unknown> {
  const answer: Record<string, unknown> = {};
  const fields = form?.fields;
  if (!fields || typeof fields !== "object") return answer;
  for (const [key, value] of Object.entries(fields as Record<string, unknown>)) {
    if (!value || typeof value !== "object") continue;
    const field = value as Record<string, unknown>;
    const options = Array.isArray(field.options) ? field.options : [];
    const first = options.find((option) =>
      Boolean(option) && typeof option === "object" && "value" in (option as Record<string, unknown>));
    if (field.default !== undefined) {
      answer[key] = field.default;
    } else if (first) {
      answer[key] = field.type === "multiselect"
        ? [(first as Record<string, unknown>).value]
        : (first as Record<string, unknown>).value;
    } else if (field.type === "boolean") {
      answer[key] = true;
    }
  }
  return answer;
}

/**
 * Forward OpenCode session events to the Responses adapter and answer the
 * permission and form prompts OpenCode raises. Runs until the stream ends or
 * `signal` aborts, which happens when the prompt settles.
 *
 * OpenCode 2.x names its native events `session.text.delta`, `session.tool.*`,
 * `permission.asked`, and `form.created`; the SDK-generated `session.next.*`
 * names are accepted too so the adapter survives a version skew.
 */
export async function consumeEvents(
  io: OpencodeHttp,
  events: V2EventStream,
  sessionID: string,
  emit: (event: OpenCodeStreamEvent) => void | Promise<void>,
  approve: ((request: OpenCodeApprovalRequest) => Promise<OpenCodeApprovalReply>) | undefined,
  signal: AbortSignal,
): Promise<void> {
  const toolNames = new Map<string, string>();
  for await (const envelope of events.stream) {
    if (signal.aborted) break;
    const type = envelope?.type;
    const data = eventData(envelope);
    if (!type || !data) continue;
    const eventSession = type === "form.created"
      ? stringField(data.form, "sessionID")
      : stringField(data, "sessionID");
    if (eventSession !== sessionID) continue;
    switch (type) {
      case "session.text.delta":
      case "session.next.text.delta":
        if (typeof data.delta === "string" && data.delta) await emit({ type: "text", delta: data.delta });
        break;
      case "session.reasoning.delta":
      case "session.next.reasoning.delta":
        if (typeof data.delta === "string" && data.delta) await emit({ type: "reasoning", delta: data.delta });
        break;
      case "session.tool.input.started": {
        const id = stringField(data, "id");
        const name = stringField(data, "name");
        if (id && name) toolNames.set(id, name);
        break;
      }
      case "session.tool.called":
      case "session.next.tool.called": {
        const callId = stringField(data, "id") ?? stringField(data, "callID") ?? "";
        const tool = stringField(data, "tool") ?? toolNames.get(callId) ?? "tool";
        if (callId) toolNames.set(callId, tool);
        await emit({ type: "tool_started", callId, tool, input: data.input });
        break;
      }
      case "session.tool.success":
      case "session.next.tool.success": {
        const callId = stringField(data, "id") ?? stringField(data, "callID") ?? "";
        await emit({ type: "tool_finished", callId, tool: toolNames.get(callId) ?? "tool", ok: true, output: toolContentText(data.content) });
        break;
      }
      case "session.tool.failed":
      case "session.next.tool.failed": {
        const callId = stringField(data, "id") ?? stringField(data, "callID") ?? "";
        await emit({ type: "tool_finished", callId, tool: toolNames.get(callId) ?? "tool", ok: false, output: errorText(data.error ?? data.message) });
        break;
      }
      case "permission.asked":
      case "permission.v2.asked": {
        if (!approve) break;
        const requestID = stringField(data, "id");
        if (!requestID) break;
        const action = stringField(data, "action") ?? "tool";
        const resources = Array.isArray(data.resources)
          ? data.resources.filter((item): item is string => typeof item === "string")
          : [];
        const reply = await approve({ kind: "permission", requestId: requestID, action, resources });
        await io.request(
          "POST",
          `/api/session/${encodeURIComponent(sessionID)}/permission/${encodeURIComponent(requestID)}/reply`,
          { decision: reply },
        );
        break;
      }
      case "question.asked":
      case "question.v2.asked": {
        if (!approve) break;
        const requestID = stringField(data, "id");
        if (!requestID) break;
        const reply = await approve({ kind: "question", requestId: requestID, questions: data.questions });
        const base = `/api/session/${encodeURIComponent(sessionID)}/question/${encodeURIComponent(requestID)}`;
        if (reply === "reject") await io.request("POST", `${base}/reject`).catch(() => {});
        else await io.request("POST", `${base}/reply`, { answers: firstAnswers(data.questions) }).catch(() => {});
        break;
      }
      case "form.created": {
        if (!approve) break;
        const form = data.form && typeof data.form === "object" ? data.form as Record<string, unknown> : undefined;
        const formID = stringField(form, "id");
        if (!formID) break;
        const reply = await approve({ kind: "form", requestId: formID, form });
        const base = `/api/session/${encodeURIComponent(sessionID)}/form/${encodeURIComponent(formID)}`;
        if (reply === "reject") {
          await io.request("DELETE", base).catch(() => {});
        } else {
          try {
            await io.request("POST", `${base}/reply`, { answer: defaultFormAnswer(form) });
          } catch {
            await io.request("DELETE", base).catch(() => {});
          }
        }
        break;
      }
    }
  }
}
