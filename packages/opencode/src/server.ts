import { timingSafeEqual, randomUUID } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { ResponseStateStore } from "./state.js";
import type {
  OpenAIError,
  OpenAIModel,
  OpenAIResponse,
  OpenCodeApprovalReply,
  OpenCodeApprovalRequest,
  OpenCodeClient,
  OpenCodeProvider,
  OpenCodeStreamEvent,
} from "./protocol.js";

const BODY_LIMIT_BYTES = 10 * 1024 * 1024;
/** How long an OpenCode permission/question may wait for a client answer. */
const APPROVAL_TIMEOUT_MS = 15 * 60 * 1000;

/**
 * Tracks in-flight OpenCode permission/question requests. The streaming client
 * sees `response.riga.approval.requested` and answers with a separate
 * `POST /v1/approvals/{id}`; the pending prompt resumes with that reply.
 */
class ApprovalRegistry {
  private readonly pending = new Map<string, (reply: OpenCodeApprovalReply) => void>();

  create(): { id: string; reply: Promise<OpenCodeApprovalReply> } {
    const id = `approval-${randomUUID()}`;
    let settle!: (reply: OpenCodeApprovalReply) => void;
    const reply = new Promise<OpenCodeApprovalReply>((resolve) => { settle = resolve; });
    const timer = setTimeout(() => { this.resolve(id, "reject"); }, APPROVAL_TIMEOUT_MS);
    timer.unref?.();
    this.pending.set(id, (value) => {
      clearTimeout(timer);
      settle(value);
    });
    return { id, reply };
  }

  resolve(id: string, reply: OpenCodeApprovalReply): boolean {
    const settle = this.pending.get(id);
    if (!settle) return false;
    this.pending.delete(id);
    settle(reply);
    return true;
  }
}

type ResponseRequest = {
  model?: unknown;
  input?: unknown;
  instructions?: unknown;
  previous_response_id?: unknown;
  conversation?: unknown;
  stream?: unknown;
  [key: string]: unknown;
};

type ResponseApiOptions = {
  apiKey?: string;
  statePath?: string;
};

type ResolvedModel = { providerID: string; modelID: string; fullID: string };
type Prompt = { text: string; system?: string };

export function createResponseApiServer(client: OpenCodeClient, options: ResponseApiOptions = {}): Server {
  const store = new ResponseStateStore(options.statePath);
  const sessionQueues = new Map<string, Promise<void>>();
  const approvals = new ApprovalRegistry();

  return createServer((request, response) => {
    void handleRequest(request, response, client, store, sessionQueues, approvals, options.apiKey)
      .catch((error: unknown) => {
        if (response.headersSent) {
          response.end();
          return;
        }
        sendError(response, 502, errorMessage(error), "server_error", undefined, "upstream_error");
      });
  });
}

async function handleRequest(
  request: IncomingMessage,
  response: ServerResponse,
  client: OpenCodeClient,
  store: ResponseStateStore,
  sessionQueues: Map<string, Promise<void>>,
  approvals: ApprovalRegistry,
  apiKey?: string,
): Promise<void> {
  const url = new URL(request.url ?? "/", "http://localhost");
  if (apiKey && !authorized(request, apiKey)) {
    sendError(response, 401, "Invalid or missing bearer token.", "authentication_error", undefined, "invalid_api_key");
    return;
  }

  // Abort in-flight upstream work when the caller goes away so a stalled turn
  // does not hold the per-session queue for subsequent requests.
  const controller = new AbortController();
  const onDisconnect = () => {
    if (!response.writableFinished) controller.abort();
  };
  request.once("aborted", onDisconnect);
  response.once("close", onDisconnect);

  if (request.method === "GET" && url.pathname === "/health") {
    sendJson(response, 200, { status: "ok", adapter: "riga-opencode" });
    return;
  }

  if (request.method === "GET" && url.pathname === "/v1/models") {
    const models = await listModels(client);
    sendJson(response, 200, { object: "list", data: models });
    return;
  }

  if (request.method === "GET" && url.pathname.startsWith("/v1/responses/")) {
    const responseId = decodeURIComponent(url.pathname.slice("/v1/responses/".length));
    const saved = await store.getResponse(responseId);
    if (!saved) {
      sendError(response, 404, `Response '${responseId}' was not found.`, "invalid_request_error", "response_id", "not_found");
      return;
    }
    sendJson(response, 200, saved);
    return;
  }

  if (request.method === "POST" && url.pathname.startsWith("/v1/approvals/")) {
    const approvalId = decodeURIComponent(url.pathname.slice("/v1/approvals/".length));
    let decision: { reply?: unknown };
    try {
      decision = await readJson(request) as { reply?: unknown };
    } catch (error) {
      sendError(response, 400, errorMessage(error), "invalid_request_error", "reply", "invalid_json");
      return;
    }
    if (!approvals.resolve(approvalId, normalizeApprovalReply(decision.reply))) {
      sendError(response, 404, `Approval '${approvalId}' is not pending.`, "invalid_request_error", "reply", "not_found");
      return;
    }
    response.writeHead(204);
    response.end();
    return;
  }

  if (request.method !== "POST" || url.pathname !== "/v1/responses") {
    sendError(response, 404, "Route not found.", "invalid_request_error", undefined, "not_found");
    return;
  }

  let body: ResponseRequest;
  try {
    body = await readJson(request);
  } catch (error) {
    const tooLarge = error instanceof Error && error.message === "request_too_large";
    sendError(response, tooLarge ? 413 : 400, tooLarge ? "Request body is too large." : errorMessage(error), "invalid_request_error", undefined, tooLarge ? "request_too_large" : "invalid_json");
    return;
  }

  if (
    body.previous_response_id !== undefined && body.previous_response_id !== null
    && body.conversation !== undefined && body.conversation !== null
  ) {
    sendError(response, 400, "previous_response_id and conversation cannot be used together.", "invalid_request_error", "conversation", "conflicting_state_parameters");
    return;
  }

  let model: ResolvedModel;
  let prompt: Prompt;
  try {
    model = await resolveModel(client, body.model);
    prompt = parsePrompt(body);
  } catch (error) {
    const invalid = error instanceof RequestValidationError;
    sendError(response, invalid ? 400 : 502, errorMessage(error), invalid ? "invalid_request_error" : "server_error", invalid ? error.param : undefined, invalid ? error.code : "upstream_error");
    return;
  }

  let sessionId: string;
  try {
    const thread = await resolveThread(body, client);
    sessionId = thread.sessionId;
  } catch (error) {
    const invalid = error instanceof RequestValidationError;
    sendError(
      response,
      invalid ? 400 : 502,
      errorMessage(error),
      invalid ? "invalid_request_error" : "server_error",
      invalid ? error.param : undefined,
      invalid ? error.code : "upstream_error",
    );
    return;
  }

  if (body.stream === true) {
    await streamResponse(response, client, store, sessionQueues, approvals, controller, model, prompt, sessionId);
    return;
  }

  try {
    const result = unwrapSdkResult(await withSessionQueue(sessionQueues, sessionId, () => client.session.prompt({
      path: { id: sessionId },
      body: {
        model: { providerID: model.providerID, modelID: model.modelID },
        parts: [{ type: "text", text: prompt.text }],
        ...(prompt.system ? { system: prompt.system } : {}),
      },
      signal: controller.signal,
    })));
    const outputText = extractAssistantText(result);
    const responseObject = makeResponse(result, outputText, model.fullID, sessionId);
    await store.recordResponse(responseObject);
    sendJson(response, 200, responseObject);
  } catch (error) {
    sendError(response, 502, errorMessage(error), "server_error", undefined, "upstream_error");
  }
}

/**
 * Stream one turn as Responses SSE. Text, reasoning, and tool activity are
 * forwarded as OpenCode produces them, so a long tool-using turn is visible
 * instead of buffered. OpenCode permission/question requests become
 * `response.riga.approval.requested` frames answered through
 * `POST /v1/approvals/{id}`, which resumes the pending prompt.
 */
async function streamResponse(
  response: ServerResponse,
  client: OpenCodeClient,
  store: ResponseStateStore,
  sessionQueues: Map<string, Promise<void>>,
  approvals: ApprovalRegistry,
  controller: AbortController,
  model: ResolvedModel,
  prompt: Prompt,
  sessionId: string,
): Promise<void> {
  response.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
    connection: "keep-alive",
  });
  const pending = {
    id: sessionId,
    object: "response",
    created_at: Math.floor(Date.now() / 1000),
    status: "in_progress",
    model: model.fullID,
    conversation: { id: sessionId },
    output: [],
    output_text: "",
  };
  writeEvent(response, "response.created", { type: "response.created", response: pending });
  writeEvent(response, "response.in_progress", { type: "response.in_progress", response: pending });

  let streamedText = "";
  const toolNames = new Map<string, string>();
  const write = (event: string, data: unknown): void => {
    if (!response.writableEnded && !response.destroyed) writeEvent(response, event, data);
  };
  const onEvent = (event: OpenCodeStreamEvent): void => {
    switch (event.type) {
      case "text":
        streamedText += event.delta;
        write("response.output_text.delta", { type: "response.output_text.delta", delta: event.delta });
        break;
      case "reasoning":
        write("response.riga.reasoning.delta", { type: "response.riga.reasoning.delta", delta: event.delta });
        break;
      case "tool_started":
        toolNames.set(event.callId, event.tool);
        write("response.riga.tool_call.started", {
          type: "response.riga.tool_call.started",
          call_id: event.callId,
          name: event.tool,
          arguments: event.input,
        });
        break;
      case "tool_finished":
        write("response.riga.tool_call.finished", {
          type: "response.riga.tool_call.finished",
          call_id: event.callId,
          name: event.tool || toolNames.get(event.callId) || "tool",
          ok: event.ok,
          output: event.output,
        });
        break;
    }
  };
  const onApproval = (approval: OpenCodeApprovalRequest): Promise<OpenCodeApprovalReply> => {
    const summary = approval.kind === "permission"
      ? `${approval.action}${approval.resources.length ? ` ${approval.resources.join(" ")}` : ""}`
      : approval.kind === "form"
        ? formSummary(approval.form)
        : "OpenCode is asking a question";
    const pendingApproval = approvals.create();
    write("response.riga.approval.requested", {
      type: "response.riga.approval.requested",
      approval_id: pendingApproval.id,
      tool: approval.kind === "permission" ? approval.action : approval.kind,
      summary,
      kind: approval.kind,
      ...(approval.kind === "question" ? { questions: approval.questions } : {}),
    });
    return pendingApproval.reply;
  };

  try {
    const result = unwrapSdkResult(await withSessionQueue(sessionQueues, sessionId, () => client.session.prompt({
      path: { id: sessionId },
      body: {
        model: { providerID: model.providerID, modelID: model.modelID },
        parts: [{ type: "text", text: prompt.text }],
        ...(prompt.system ? { system: prompt.system } : {}),
      },
      signal: controller.signal,
      onEvent,
      onApproval,
    })));
    const finalText = extractAssistantText(result);
    // Prefer what was actually streamed: a multi-step turn can emit prose
    // between tool calls, and the client has already seen all of it.
    const outputText = streamedText.length > 0 ? streamedText : finalText;
    const responseObject = makeResponse(result, outputText, model.fullID, sessionId);
    await store.recordResponse(responseObject);
    if (outputText.length > 0) {
      // A provider that reported no incremental text still needs one delta.
      if (streamedText.length === 0) {
        write("response.output_text.delta", { type: "response.output_text.delta", delta: outputText });
      }
      write("response.output_text.done", {
        type: "response.output_text.done",
        item_id: responseObject.output[0]?.id,
        output_index: 0,
        content_index: 0,
        text: outputText,
      });
    }
    write("response.completed", { type: "response.completed", response: responseObject });
  } catch (error) {
    write("response.failed", {
      type: "response.failed",
      response: {
        id: sessionId,
        object: "response",
        status: "failed",
        model: model.fullID,
        error: { message: errorMessage(error), type: "server_error", code: "upstream_error" },
      },
    });
  } finally {
    if (!response.writableEnded) response.end();
  }
}

function normalizeApprovalReply(value: unknown): OpenCodeApprovalReply {
  return value === "once" || value === "always" ? value : "reject";
}

/** A one-line description of a form (the modern `question` tool). */
function formSummary(form: unknown): string {
  if (!form || typeof form !== "object") return "OpenCode is asking for input";
  const record = form as { title?: unknown; fields?: unknown };
  const title = typeof record.title === "string" && record.title.trim()
    ? record.title.trim()
    : "OpenCode is asking for input";
  const count = record.fields && typeof record.fields === "object"
    ? Object.keys(record.fields as Record<string, unknown>).length
    : 0;
  return count > 0 ? `${title} (${count} field${count === 1 ? "" : "s"})` : title;
}

function authorized(request: IncomingMessage, expected: string): boolean {
  const header = request.headers.authorization;
  if (!header?.startsWith("Bearer ")) return false;
  const actual = Buffer.from(header.slice("Bearer ".length));
  const wanted = Buffer.from(expected);
  return actual.length === wanted.length && timingSafeEqual(actual, wanted);
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function listModels(client: OpenCodeClient): Promise<OpenAIModel[]> {
  const providers = await settleCatalog(client, hasModels);
  return providers
    .flatMap((provider: OpenCodeProvider) => Object.values(provider.models ?? {}).map((model) => ({
      id: `${provider.id}/${model.id}`,
      object: "model" as const,
      created: 0,
      owned_by: provider.id,
      name: model.name,
    })))
    .sort((left, right) => left.owned_by.localeCompare(right.owned_by) || left.id.localeCompare(right.id));
}

async function listProviders(client: OpenCodeClient): Promise<OpenCodeProvider[]> {
  const catalog = unwrapSdkResult(await client.config.providers());
  if (!catalog || typeof catalog !== "object" || !Array.isArray((catalog as { providers?: unknown }).providers)) {
    throw new Error("OpenCode did not return a valid provider catalog.");
  }
  return (catalog as { providers: OpenCodeProvider[] }).providers;
}

const CATALOG_POLL_MS = 150;
const CATALOG_TIMEOUT_MS = 30_000;

function hasModels(providers: OpenCodeProvider[]): boolean {
  return providers.some((provider) => Object.keys(provider.models ?? {}).length > 0);
}

/** The provider/model match for a `provider/model` (or bare `model`) id. */
function matchModel(
  providers: OpenCodeProvider[],
  fullID: string,
): { provider: OpenCodeProvider; model: { id: string } } | undefined {
  const separator = fullID.indexOf("/");
  if (separator > 0) {
    const providerID = fullID.slice(0, separator);
    const modelID = fullID.slice(separator + 1);
    const provider = providers.find((candidate) => candidate.id === providerID);
    const model = provider?.models?.[modelID]
      ?? Object.values(provider?.models ?? {}).find((candidate) => candidate.id === modelID);
    return provider && model ? { provider, model } : undefined;
  }
  const matches = providers.flatMap((provider) => Object.values(provider.models ?? {})
    .filter((candidate) => candidate.id === fullID)
    .map((model) => ({ provider, model })));
  return matches.length === 1 ? matches[0] : undefined;
}

/**
 * Read the provider catalog, tolerating OpenCode's cold-start behavior: the
 * first read triggers the upstream provider/model load and returns the
 * pre-settlement (often empty) snapshot, while later reads include it. Retry
 * until the predicate holds or the catalog is non-empty; an upstream error is
 * surfaced rather than retried.
 */
async function settleCatalog(
  client: OpenCodeClient,
  ready: (providers: OpenCodeProvider[]) => boolean,
): Promise<OpenCodeProvider[]> {
  const deadline = Date.now() + CATALOG_TIMEOUT_MS;
  for (;;) {
    const providers = await listProviders(client);
    if (ready(providers) || providers.length > 0 || Date.now() >= deadline) {
      return providers;
    }
    await delay(CATALOG_POLL_MS);
  }
}

async function resolveModel(client: OpenCodeClient, modelValue: unknown): Promise<ResolvedModel> {
  if (typeof modelValue !== "string" || !modelValue.trim()) {
    throw new RequestValidationError("model is required and must be a provider/model ID.", "model", "model_required");
  }
  const fullID = modelValue.trim();
  const providers = await settleCatalog(client, (list) => matchModel(list, fullID) !== undefined);
  const match = matchModel(providers, fullID);
  if (!match) {
    throw new RequestValidationError(`Model '${fullID}' is not available. Use a model ID returned by GET /v1/models.`, "model", "model_not_found");
  }
  return { providerID: match.provider.id, modelID: match.model.id, fullID: `${match.provider.id}/${match.model.id}` };
}

function parsePrompt(body: ResponseRequest): Prompt {
  const systemParts: string[] = [];
  if (typeof body.instructions === "string" && body.instructions.trim()) systemParts.push(body.instructions.trim());
  const userParts: string[] = [];
  const suppliedHistory: string[] = [];
  const input = body.input;
  if (typeof input === "string") {
    userParts.push(input);
  } else if (Array.isArray(input)) {
    for (const item of input) {
      if (!item || typeof item !== "object") throw new RequestValidationError("Each input item must be an object.", "input", "invalid_input");
      const record = item as Record<string, unknown>;
      if (record.type === "input_text") {
        userParts.push(textContent(record.text));
        continue;
      }
      if (record.type && record.type !== "message") {
        throw new RequestValidationError(`Input item type '${String(record.type)}' is not supported by the OpenCode text adapter.`, "input", "unsupported_input_type");
      }
      const role = typeof record.role === "string" ? record.role : "user";
      const content = textContent(record.content);
      if (role === "system" || role === "developer") systemParts.push(content);
      else if (role === "user") userParts.push(content);
      else if (role === "assistant") suppliedHistory.push(`Assistant context: ${content}`);
      else throw new RequestValidationError(`Input role '${role}' is not supported.`, "input", "unsupported_role");
    }
  } else {
    throw new RequestValidationError("input must be a string or an array of text messages.", "input", "invalid_input");
  }
  const text = [...suppliedHistory, ...userParts].filter((part) => part.trim()).join("\n\n");
  if (!text.trim()) throw new RequestValidationError("input must contain non-empty text.", "input", "empty_input");
  return { text, ...(systemParts.length ? { system: systemParts.join("\n\n") } : {}) };
}

function textContent(value: unknown): string {
  if (typeof value === "string") return value;
  if (!Array.isArray(value)) throw new RequestValidationError("Message content must be text or an array of text parts.", "input", "invalid_input");
  const parts: string[] = [];
  for (const part of value) {
    if (typeof part === "string") parts.push(part);
    else if (part && typeof part === "object") {
      const record = part as Record<string, unknown>;
      if ((record.type === "input_text" || record.type === "text") && typeof record.text === "string") parts.push(record.text);
      else throw new RequestValidationError("Only text input parts are supported by the OpenCode adapter.", "input", "unsupported_input_type");
    } else throw new RequestValidationError("Message content contains an invalid part.", "input", "invalid_input");
  }
  return parts.join("");
}

async function resolveThread(
  body: ResponseRequest,
  client: OpenCodeClient,
): Promise<{ sessionId: string }> {
  if (body.previous_response_id !== undefined && body.previous_response_id !== null) {
    if (typeof body.previous_response_id !== "string" || !body.previous_response_id.trim()) {
      throw new RequestValidationError("previous_response_id must be a non-empty session ID.", "previous_response_id", "invalid_response_id");
    }
    return { sessionId: body.previous_response_id };
  }
  if (body.conversation !== undefined && body.conversation !== null) {
    const value = typeof body.conversation === "string"
      ? body.conversation
      : typeof body.conversation === "object" && body.conversation !== null
        ? (body.conversation as Record<string, unknown>).id
        : undefined;
    if (typeof value !== "string" || !value.trim()) throw new RequestValidationError("conversation must be a session ID or an object containing id.", "conversation", "invalid_conversation");
    return { sessionId: value };
  }
  const created = unwrapSdkResult(await client.session.create({ body: { title: "OpenAI Responses conversation" } }));
  const sessionId = created && typeof created === "object" ? (created as { id?: unknown }).id : undefined;
  if (typeof sessionId !== "string" || !sessionId) throw new Error("OpenCode did not return a session ID.");
  return { sessionId };
}

function unwrapSdkResult(value: unknown): unknown {
  if (!value || typeof value !== "object") return value;
  const result = value as Record<string, unknown>;
  if (result.error !== undefined && result.error !== null) {
    const error = result.error;
    if (error instanceof Error) throw error;
    const message = error && typeof error === "object" && typeof (error as { message?: unknown }).message === "string"
      ? (error as { message: string }).message
      : JSON.stringify(error);
    throw new Error(message || "OpenCode SDK request failed.");
  }
  return Object.hasOwn(result, "data") ? result.data : value;
}

function extractAssistantText(result: unknown): string {
  result = unwrapSdkResult(result);
  if (!result || typeof result !== "object") return "";
  const record = result as Record<string, unknown>;
  const info = (record.info && typeof record.info === "object" ? record.info : record) as Record<string, unknown>;
  const error = info.error as { data?: { message?: string }; message?: string } | undefined;
  if (error) throw new Error(error.data?.message ?? error.message ?? "OpenCode returned an assistant error.");
  const parts = Array.isArray(record.parts) ? record.parts : [];
  return parts.flatMap((part) => {
    if (!part || typeof part !== "object") return [];
    const value = part as Record<string, unknown>;
    return value.type === "text" && typeof value.text === "string" && value.ignored !== true ? [value.text] : [];
  }).join("");
}

function makeResponse(result: unknown, outputText: string, model: string, sessionId: string): OpenAIResponse {
  const record = result && typeof result === "object" ? result as Record<string, unknown> : {};
  const info = (record.info && typeof record.info === "object" ? record.info : record) as Record<string, unknown>;
  const tokens = info.tokens && typeof info.tokens === "object" ? info.tokens as Record<string, unknown> : {};
  const cache = tokens.cache && typeof tokens.cache === "object" ? tokens.cache as Record<string, unknown> : {};
  const inputTokens = finiteNumber(tokens.input);
  const outputTokens = finiteNumber(tokens.output);
  const reasoningTokens = finiteNumber(tokens.reasoning);
  const messageId = typeof info.id === "string" ? info.id : `msg_${randomUUID().replaceAll("-", "")}`;
  return {
    id: sessionId,
    object: "response",
    created_at: Math.floor(Date.now() / 1000),
    status: "completed",
    model,
    conversation: { id: sessionId },
    output: [{
      id: messageId,
      type: "message",
      status: "completed",
      role: "assistant",
      content: [{ type: "output_text", text: outputText, annotations: [] }],
    }],
    output_text: outputText,
    usage: {
      input_tokens: inputTokens,
      input_tokens_details: { cached_tokens: finiteNumber(cache.read) },
      output_tokens: outputTokens,
      output_tokens_details: { reasoning_tokens: reasoningTokens },
      total_tokens: inputTokens + outputTokens,
    },
  };
}

function finiteNumber(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

async function withSessionQueue<T>(queues: Map<string, Promise<void>>, sessionId: string, run: () => Promise<T>): Promise<T> {
  const previous = queues.get(sessionId) ?? Promise.resolve();
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const queued = previous.then(() => gate);
  queues.set(sessionId, queued);
  await previous;
  try {
    return await run();
  } finally {
    release();
    if (queues.get(sessionId) === queued) queues.delete(sessionId);
  }
}

function writeEvent(response: ServerResponse, event: string, data: unknown): void {
  response.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
}

async function readJson(request: IncomingMessage): Promise<ResponseRequest> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of request) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    size += buffer.byteLength;
    if (size > BODY_LIMIT_BYTES) throw new Error("request_too_large");
    chunks.push(buffer);
  }
  const parsed: unknown = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("Request body must be a JSON object.");
  return parsed as ResponseRequest;
}

function sendJson(response: ServerResponse, status: number, body: unknown): void {
  if (response.writableEnded || response.destroyed) return;
  response.writeHead(status, { "content-type": "application/json; charset=utf-8" });
  response.end(JSON.stringify(body));
}

function sendError(response: ServerResponse, status: number, message: string, type: OpenAIError["error"]["type"], param?: string, code?: string): void {
  sendJson(response, status, { error: { message, type, ...(param ? { param } : {}), ...(code ? { code } : {}) } } satisfies OpenAIError);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

class RequestValidationError extends Error {
  constructor(message: string, readonly param: string, readonly code: string) { super(message); }
}
