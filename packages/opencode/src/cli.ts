#!/usr/bin/env node
import { createOpencodeClient } from "@opencode-ai/sdk/v2";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createServer as createProbeServer } from "node:net";
import { env, exit } from "node:process";
import { createResponseApiServer } from "./server.js";
import { opencodeConfigContent } from "./config.js";
import { consumeEvents, createOpencodeHttp, type OpencodeHttp, type V2EventStream } from "./events.js";
import type { OpenCodeClient, OpenCodeProvider } from "./protocol.js";

const SERVER_START_TIMEOUT_MS = 15_000;
const PROMPT_TIMEOUT_MS = 30 * 60 * 1000;
const PROMPT_POLL_INTERVAL_MS = 250;
const SESSION_IDLE_TIMEOUT_MS = 15_000;

function positivePort(value: string | undefined, fallback: number): number {
  if (!value) return fallback;
  const port = Number(value);
  if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error(`Invalid port: ${value}`);
  return port;
}

function isLoopbackHost(host: string): boolean {
  return ["localhost", "127.0.0.1", "::1", "[::1]"].includes(host.toLowerCase());
}

// The OpenCode v2 HTTP API wraps payloads in a `{ data, location }` envelope and
// returns the Hey API `{ data, error }` result from the SDK. These types describe
// the subset of that surface this adapter uses.
type SdkResult<T> = { data?: T; error?: unknown };
type ProviderInfo = { id: string; name?: string };
type ModelInfo = { modelID: string; providerID: string; name?: string };
type SessionInfo = { id: string };
type AssistantContent = { type?: string; text?: string };
type AssistantTokens = {
  input?: number;
  output?: number;
  reasoning?: number;
  cache?: { read?: number; write?: number };
};
type AssistantMessage = {
  id: string;
  type?: string;
  time?: { created?: number; completed?: number };
  content?: AssistantContent[];
  tokens?: AssistantTokens;
  error?: { message?: string } | string | null;
  finish?: string;
};
type InboxUser = { id?: string; time?: { created?: number } };
type V2Client = {
  v2: {
    provider: { list(): Promise<SdkResult<{ data?: ProviderInfo[] }>> };
    model: { list(): Promise<SdkResult<{ data?: ModelInfo[] }>> };
    event: {
      subscribe(options?: { signal?: AbortSignal; sseMaxRetryAttempts?: number }): Promise<V2EventStream>;
    };
    session: {
      create(body: {
        $body_title?: string;
        model?: { id: string; providerID: string };
      }): Promise<SdkResult<{ data?: SessionInfo }>>;
      switchModel(args: {
        sessionID: string;
        model: { id: string; providerID: string };
      }): Promise<SdkResult<unknown>>;
      prompt(args: { sessionID: string; $body_text: string }): Promise<SdkResult<InboxUser>>;
      interrupt(args: { sessionID: string }): Promise<SdkResult<unknown>>;
      messages(args: {
        sessionID: string;
        limit?: number;
        order?: "asc" | "desc";
      }): Promise<SdkResult<{ data?: AssistantMessage[] }>>;
      active(): Promise<SdkResult<{ data?: Record<string, unknown> }>>;
    };
  };
};

function sdkError(error: unknown): Error {
  if (error instanceof Error) return error;
  if (error && typeof error === "object") {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message) return new Error(message);
    return new Error(JSON.stringify(error));
  }
  return new Error(String(error));
}

function unwrap<T>(result: SdkResult<T>): T {
  if (result && result.error !== undefined && result.error !== null) throw sdkError(result.error);
  return result.data as T;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// OpenCode now exposes separate provider and model catalogs; merge them into the
// `provider -> models` shape the Responses adapter expects.
async function loadProviders(sdk: V2Client): Promise<OpenCodeProvider[]> {
  const [providersResult, modelsResult] = await Promise.all([sdk.v2.provider.list(), sdk.v2.model.list()]);
  const catalog = new Map<string, OpenCodeProvider>();
  for (const provider of unwrap(providersResult)?.data ?? []) {
    if (!catalog.has(provider.id)) {
      catalog.set(provider.id, { id: provider.id, name: provider.name ?? provider.id, models: {} });
    }
  }
  for (const model of unwrap(modelsResult)?.data ?? []) {
    let provider = catalog.get(model.providerID);
    if (!provider) {
      provider = { id: model.providerID, name: model.providerID, models: {} };
      catalog.set(provider.id, provider);
    }
    provider.models[model.modelID] = { id: model.modelID, name: model.name ?? model.modelID };
  }
  return [...catalog.values()];
}

function assistantError(message: AssistantMessage): Error | null {
  const error = message.error;
  if (typeof error === "string" && error) return new Error(error);
  if (error && typeof error === "object" && typeof error.message === "string") return new Error(error.message);
  if (message.finish === "error") return new Error("OpenCode reported an assistant error.");
  return null;
}

function assertNotAborted(signal?: AbortSignal): void {
  if (signal?.aborted) throw new Error("Client disconnected before OpenCode finished the response.");
}

async function isSessionRunning(sdk: V2Client, sessionID: string): Promise<boolean> {
  try {
    const active = unwrap(await sdk.v2.session.active())?.data ?? {};
    return Object.prototype.hasOwnProperty.call(active, sessionID);
  } catch {
    return false;
  }
}

/**
 * OpenCode serializes execution per session, so a session left "running" by an
 * earlier aborted or failed turn blocks every later prompt. Interrupt it and
 * wait for the session to become idle before sending a new prompt.
 */
async function ensureSessionIdle(sdk: V2Client, sessionID: string, signal?: AbortSignal): Promise<void> {
  if (!(await isSessionRunning(sdk, sessionID))) return;
  try {
    unwrap(await sdk.v2.session.interrupt({ sessionID }));
  } catch {
    // Best effort; the bounded wait below decides whether we can continue.
  }
  const deadline = Date.now() + SESSION_IDLE_TIMEOUT_MS;
  while (Date.now() < deadline) {
    assertNotAborted(signal);
    if (!(await isSessionRunning(sdk, sessionID))) return;
    await delay(PROMPT_POLL_INTERVAL_MS);
  }
  throw new Error("OpenCode session is busy and could not be interrupted.");
}

async function waitForAssistant(
  sdk: V2Client,
  sessionID: string,
  since: number,
  signal: AbortSignal | undefined,
  deadline: number,
): Promise<AssistantMessage> {
  while (Date.now() < deadline) {
    assertNotAborted(signal);
    const result = await sdk.v2.session.messages({ sessionID, limit: 50, order: "desc" });
    const assistant = (unwrap(result)?.data ?? []).find((message) =>
      message.type === "assistant"
      && typeof message.time?.created === "number" && message.time.created >= since
      && typeof message.time?.completed === "number"
      // Tool calls finish with "tool-calls" and are followed by more steps; only a
      // settled assistant message from an idle session is the final answer.
      && message.finish !== "tool-calls",
    );
    if (assistant) {
      const error = assistantError(assistant);
      if (error) throw error;
      if (!(await isSessionRunning(sdk, sessionID))) return assistant;
    }
    await delay(PROMPT_POLL_INTERVAL_MS);
  }
  throw new Error("Timed out waiting for OpenCode to finish the response.");
}

/**
 * Adapt the OpenCode v2 client to the narrow `OpenCodeClient` surface used by the
 * Responses adapter. v2 executes prompts asynchronously, so a prompt is admitted
 * and then the resulting assistant message is awaited.
 */
function createOpencodeAdapter(sdk: V2Client, io: OpencodeHttp): OpenCodeClient {
  return {
    config: {
      providers: async () => ({ providers: await loadProviders(sdk) }),
    },
    session: {
      create: async (args) => {
        const session = unwrap(await sdk.v2.session.create({ $body_title: args.body.title }))?.data;
        if (!session?.id) throw new Error("OpenCode did not return a session ID.");
        return { id: session.id };
      },
      prompt: async (args) => {
        const sessionID = args.path.id;
        const signal = args.signal;
        assertNotAborted(signal);
        const onAbort = () => {
          void sdk.v2.session.interrupt({ sessionID }).catch(() => {});
        };
        signal?.addEventListener("abort", onAbort, { once: true });
        const eventsAbort = new AbortController();
        let pump: Promise<void> | undefined;
        try {
          await ensureSessionIdle(sdk, sessionID, signal);
          unwrap(await sdk.v2.session.switchModel({
            sessionID,
            model: { id: args.body.model.modelID, providerID: args.body.model.providerID },
          }));
          const parts = args.body.parts.map((part) => part.text).filter((text) => text.length > 0);
          const text = args.body.system ? [args.body.system, ...parts].join("\n\n") : parts.join("\n\n");
          if (args.onEvent || args.onApproval) {
            try {
              const events = await sdk.v2.event.subscribe({
                signal: eventsAbort.signal,
                sseMaxRetryAttempts: 3,
              });
              pump = consumeEvents(io, events, sessionID, args.onEvent ?? (() => {}), args.onApproval, eventsAbort.signal);
            } catch {
              // Event streaming is best-effort; the turn still completes buffered.
            }
          }
          const inbox = unwrap(await sdk.v2.session.prompt({ sessionID, $body_text: text }));
          const since = typeof inbox?.time?.created === "number" ? inbox.time.created : Date.now();
          const assistant = await waitForAssistant(sdk, sessionID, since, signal, Date.now() + PROMPT_TIMEOUT_MS);
          const output = (assistant.content ?? [])
            .filter((part) => part.type === "text" && typeof part.text === "string")
            .map((part) => part.text as string)
            .join("");
          return { info: { id: assistant.id, tokens: assistant.tokens ?? {} }, parts: [{ type: "text", text: output }] };
        } finally {
          eventsAbort.abort();
          if (pump) await pump.catch(() => {});
          signal?.removeEventListener("abort", onAbort);
        }
      },
    },
  };
}

type LaunchedServer = { url: string; password: string; close(): void };

/** Ask the OS for a free loopback port, so a stale server cannot block startup. */
async function freePort(host: string): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = createProbeServer();
    probe.unref();
    probe.on("error", reject);
    probe.listen(0, host, () => {
      const address = probe.address();
      const port = typeof address === "object" && address ? address.port : 0;
      probe.close(() => resolve(port));
    });
  });
}

/**
 * Start the OpenCode server, avoiding a fixed port by default. The old fixed
 * `4096` collides with an OpenCode server a user already runs (or a stale one
 * from a previous adapter), which used to abort the whole adapter. A pinned
 * `OPENCODE_PORT` is honoured and its failure surfaced; otherwise a free port is
 * chosen and retried if it is raced away.
 */
async function launchOpencodeServer(host: string, pinnedPort?: number): Promise<LaunchedServer> {
  const attempts = pinnedPort === undefined ? 4 : 1;
  let port = pinnedPort ?? (await freePort(host));
  let lastError: unknown;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const server = await startOpencodeServer(host, port);
      if (pinnedPort === undefined && attempt > 0) {
        console.log(`OpenCode server started on fallback port ${port}.`);
      }
      return server;
    } catch (error) {
      lastError = error;
      port = await freePort(host);
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}

/**
 * Spawn `opencode serve` and wait for it to bind. The official SDK helper waits
 * for a legacy "opencode server listening" banner that v2 no longer prints, and
 * it does not surface the server password required by the v2 API, so we launch
 * the server ourselves and authenticate with a known password.
 */
async function startOpencodeServer(hostname: string, port: number): Promise<LaunchedServer> {
  const password = randomBytes(32).toString("base64url");
  const proc = spawn("opencode", ["serve", `--hostname=${hostname}`, `--port=${port}`], {
    env: {
      ...env,
      OPENCODE_PASSWORD: password,
      OPENCODE_CONFIG_CONTENT: opencodeConfigContent(env.OPENCODE_CONFIG_CONTENT),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });

  const url = await new Promise<string>((resolve, reject) => {
    let output = "";
    let settled = false;
    const finish = (error: Error | null, value?: string) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) reject(error);
      else resolve(value!);
    };
    const timer = setTimeout(() => {
      proc.kill();
      finish(new Error(`Timed out waiting for the OpenCode server to start after ${SERVER_START_TIMEOUT_MS}ms.`));
    }, SERVER_START_TIMEOUT_MS);
    proc.stdout?.on("data", (chunk: Buffer) => {
      output += chunk.toString();
      const match = output.match(/(?:opencode )?server listening on (https?:\/\/\S+)/);
      if (match) finish(null, match[1]);
    });
    proc.stderr?.on("data", (chunk: Buffer) => {
      output += chunk.toString();
    });
    proc.on("error", (error) => finish(error));
    proc.on("exit", (code) => {
      finish(new Error(`OpenCode server exited with code ${code}${output.trim() ? `\n${output.trim()}` : ""}`));
    });
  });

  return { url, password, close: () => proc.kill() };
}

function authorizationHeader(password: string): string {
  return `Basic ${Buffer.from(`opencode:${password}`).toString("base64")}`;
}

async function main(): Promise<void> {
  const apiHost = env.RIGA_OPENCODE_HOST ?? "127.0.0.1";
  const apiPort = positivePort(env.RIGA_OPENCODE_PORT, 8788);
  if (!isLoopbackHost(apiHost) && !env.RIGA_OPENCODE_API_KEY) {
    throw new Error("RIGA_OPENCODE_API_KEY is required when binding the Responses API outside localhost/loopback.");
  }
  const opencodeHost = env.OPENCODE_HOST ?? "127.0.0.1";
  const pinnedPort = env.OPENCODE_PORT ? positivePort(env.OPENCODE_PORT, 4096) : undefined;

  const owned = env.OPENCODE_BASE_URL ? null : await launchOpencodeServer(opencodeHost, pinnedPort);
  const baseUrl = owned?.url ?? env.OPENCODE_BASE_URL!;
  const password = owned?.password ?? env.OPENCODE_PASSWORD;
  const sdk = createOpencodeClient({
    baseUrl,
    ...(password ? { headers: { authorization: authorizationHeader(password) } } : {}),
  }) as unknown as V2Client;
  const io = createOpencodeHttp(baseUrl, password ? authorizationHeader(password) : undefined);
  const client = createOpencodeAdapter(sdk, io);
  const server = createResponseApiServer(client, {
    apiKey: env.RIGA_OPENCODE_API_KEY,
    statePath: env.RIGA_OPENCODE_STATE_PATH,
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(apiPort, apiHost, () => {
      server.off("error", reject);
      resolve();
    });
  });

  const address = server.address();
  const boundPort = address && typeof address === "object" ? address.port : apiPort;
  console.log(`RIGA OpenCode Responses API listening at http://${apiHost}:${boundPort}/v1`);
  if (owned) console.log(`OpenCode server running at ${owned.url}`);
  if (env.RIGA_OPENCODE_API_KEY) console.log("Bearer-token authentication is enabled.");

  let closing = false;
  const close = () => {
    if (closing) return;
    closing = true;
    server.close((error) => {
      owned?.close();
      if (error) {
        console.error(error);
        exit(1);
      }
      exit(0);
    });
    server.closeAllConnections();
  };
  process.on("SIGINT", close);
  process.on("SIGTERM", close);
}

void main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : error);
  exit(1);
});
