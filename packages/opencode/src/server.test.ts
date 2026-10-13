import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createResponseApiServer } from "./server.js";
import type { OpenCodeClient } from "./protocol.js";

const servers: Array<{ close: () => Promise<void> }> = [];
const tempDirs: string[] = [];

afterEach(async () => {
  await Promise.all(servers.splice(0).map((server) => server.close()));
  await Promise.all(tempDirs.splice(0).map((path) => rm(path, { recursive: true, force: true })));
});

function fakeClient() {
  let sessionNumber = 0;
  let promptNumber = 0;
  const promptCalls: Array<{ path: { id: string }; body: { model: { providerID: string; modelID: string }; parts: Array<{ type: "text"; text: string }>; system?: string } }> = [];
  const client = {
    config: {
      providers: vi.fn(async () => ({
        providers: [
          { id: "provider-a", name: "Provider A", models: { "model-x": { id: "model-x", name: "Model X" } } },
          { id: "provider-b", name: "Provider B", models: { "model-y": { id: "model-y", name: "Model Y" } } },
        ],
      })),
    },
    session: {
      create: vi.fn(async () => ({ id: `session-${++sessionNumber}` })),
      prompt: vi.fn(async (args: (typeof promptCalls)[number]) => {
        promptCalls.push(args);
        return {
          info: { id: `message-${++promptNumber}`, tokens: { input: 7, output: 5, reasoning: 2, cache: { read: 1, write: 0 } } },
          parts: [{ type: "text", text: `reply-${promptNumber}` }],
        };
      }),
    },
  };
  return { client: client as unknown as OpenCodeClient, promptCalls, sessionCreate: client.session.create };
}

async function startServer(client: OpenCodeClient, options: { apiKey?: string; statePath?: string } = {}) {
  const dir = await mkdtemp(join(tmpdir(), "riga-opencode-"));
  tempDirs.push(dir);
  const statePath = options.statePath ?? join(dir, "responses.json");
  const server = createResponseApiServer(client, { ...options, statePath });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolve());
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Server did not bind a TCP address");
  servers.push({ close: () => new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve())) });
  return { baseUrl: `http://127.0.0.1:${address.port}`, statePath };
}

async function postResponse(baseUrl: string, body: Record<string, unknown>, apiKey?: string) {
  return fetch(`${baseUrl}/v1/responses`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(apiKey ? { authorization: `Bearer ${apiKey}` } : {}),
    },
    body: JSON.stringify(body),
  });
}

describe("OpenCode Responses API adapter", () => {
  it("lists configured provider models using OpenCode provider/model IDs", async () => {
    const { client } = fakeClient();
    const { baseUrl } = await startServer(client);
    const response = await fetch(`${baseUrl}/v1/models`);
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      object: "list",
      data: [
        { id: "provider-a/model-x", object: "model", created: 0, owned_by: "provider-a", name: "Model X" },
        { id: "provider-b/model-y", object: "model", created: 0, owned_by: "provider-b", name: "Model Y" },
      ],
    });
  });

  it("unwraps the official SDK response envelope for provider, session, and prompt calls", async () => {
    const { client: rawClient } = fakeClient();
    const client = {
      config: {
        providers: async () => ({ data: await rawClient.config.providers(), error: undefined }),
      },
      session: {
        create: async (args: Parameters<typeof rawClient.session.create>[0]) => ({
          data: await rawClient.session.create(args),
          error: undefined,
        }),
        prompt: async (args: Parameters<typeof rawClient.session.prompt>[0]) => ({
          data: await rawClient.session.prompt(args),
          error: undefined,
        }),
      },
    } as unknown as OpenCodeClient;
    const { baseUrl } = await startServer(client);

    const catalog = await fetch(`${baseUrl}/v1/models`);
    expect(catalog.status).toBe(200);
    expect((await catalog.json() as { data: Array<{ id: string }> }).data.map(({ id }) => id))
      .toEqual(["provider-a/model-x", "provider-b/model-y"]);

    const response = await postResponse(baseUrl, { model: "provider-a/model-x", input: "Wrapped SDK result" });
    expect(response.status).toBe(200);
    expect((await response.json() as { output_text: string }).output_text).toBe("reply-1");
  });

  it("reports OpenCode SDK provider errors as upstream failures", async () => {
    const { client: rawClient } = fakeClient();
    const client = {
      ...rawClient,
      config: {
        providers: async () => ({ data: undefined, error: { message: "OpenCode is unavailable" } }),
      },
    } as unknown as OpenCodeClient;
    const { baseUrl } = await startServer(client);
    const response = await postResponse(baseUrl, { model: "provider-a/model-x", input: "hello" });
    expect(response.status).toBe(502);
    expect((await response.json() as { error: { code: string } }).error.code).toBe("upstream_error");
  });

  it("creates a response and continues the same OpenCode session through previous_response_id", async () => {
    const { client, promptCalls, sessionCreate } = fakeClient();
    const { baseUrl } = await startServer(client);
    const firstHttp = await postResponse(baseUrl, { model: "provider-a/model-x", input: "Hello" });
    expect(firstHttp.status).toBe(200);
    const first = await firstHttp.json() as { id: string; conversation: { id: string }; output_text: string; usage: { total_tokens: number } };
    expect(first.id).toBe("session-1");
    expect(first.conversation.id).toBe(first.id);
    expect(first.output_text).toBe("reply-1");
    expect(first.usage.total_tokens).toBe(12);

    const secondHttp = await postResponse(baseUrl, {
      model: "provider-a/model-x",
      input: [{ role: "user", content: [{ type: "input_text", text: "Continue" }] }],
      previous_response_id: first.id,
    });
    expect(secondHttp.status).toBe(200);
    const second = await secondHttp.json() as { id: string; conversation: { id: string } };
    expect(second.id).toBe(first.id);
    expect(second.conversation.id).toBe(first.conversation.id);
    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(promptCalls.map((call) => call.path.id)).toEqual(["session-1", "session-1"]);
    expect(promptCalls[0]?.body.model).toEqual({ providerID: "provider-a", modelID: "model-x" });
    expect(promptCalls[1]?.body.parts[0]?.text).toBe("Continue");
  });

  it("continues the same OpenCode session across proxy restarts using the direct session ID", async () => {
    const { client, promptCalls, sessionCreate } = fakeClient();
    const dir = await mkdtemp(join(tmpdir(), "riga-opencode-state-"));
    tempDirs.push(dir);
    const statePath = join(dir, "responses.json");
    const firstServer = await startServer(client, { statePath });
    const firstHttp = await postResponse(firstServer.baseUrl, { model: "provider-b/model-y", input: "First turn" });
    const first = await firstHttp.json() as { id: string; conversation: { id: string } };
    const closeFirst = servers.pop();
    await closeFirst?.close();

    const secondServer = await startServer(client, { statePath });
    const retrieved = await fetch(`${secondServer.baseUrl}/v1/responses/${encodeURIComponent(first.id)}`);
    expect(retrieved.status).toBe(200);
    expect((await retrieved.json() as { id: string }).id).toBe(first.id);
    const closeSecond = servers.pop();
    await closeSecond?.close();

    // This proxy has an empty response cache. Continuation must still work
    // because previous_response_id is the upstream session ID itself.
    const thirdServer = await startServer(client, { statePath: join(dir, "empty-response-cache.json") });
    const secondHttp = await postResponse(thirdServer.baseUrl, {
      model: "provider-b/model-y",
      input: "Second turn",
      previous_response_id: first.id,
    });
    expect(secondHttp.status).toBe(200);
    expect((await secondHttp.json() as { id: string }).id).toBe(first.id);
    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(promptCalls.map((call) => call.path.id)).toEqual(["session-1", "session-1"]);
  });

  it("rejects conflicting or malformed state identifiers using OpenAI-style errors", async () => {
    const { client } = fakeClient();
    const { baseUrl } = await startServer(client);
    const conflict = await postResponse(baseUrl, {
      model: "provider-a/model-x",
      input: "x",
      previous_response_id: "resp_old",
      conversation: "conv_old",
    });
    expect(conflict.status).toBe(400);
    expect((await conflict.json() as { error: { code: string } }).error.code).toBe("conflicting_state_parameters");

    const malformed = await postResponse(baseUrl, { model: "provider-a/model-x", input: "x", previous_response_id: "" });
    expect(malformed.status).toBe(400);
    expect((await malformed.json() as { error: { param: string } }).error.param).toBe("previous_response_id");
  });

  it("requires a valid model and rejects models not returned by the provider catalog", async () => {
    const { client } = fakeClient();
    const { baseUrl } = await startServer(client);
    const missing = await postResponse(baseUrl, { input: "hello" });
    expect(missing.status).toBe(400);
    expect((await missing.json() as { error: { param: string } }).error.param).toBe("model");

    const unknown = await postResponse(baseUrl, { model: "unknown/model", input: "hello" });
    expect(unknown.status).toBe(400);
    expect((await unknown.json() as { error: { code: string } }).error.code).toBe("model_not_found");
  });

  it("supports bearer-token protection and emits Responses SSE events", async () => {
    const { client } = fakeClient();
    const { baseUrl } = await startServer(client, { apiKey: "test-secret" });
    const unauthorized = await postResponse(baseUrl, { model: "provider-a/model-x", input: "hello" });
    expect(unauthorized.status).toBe(401);

    const streamed = await postResponse(baseUrl, { model: "provider-a/model-x", input: "hello", stream: true }, "test-secret");
    expect(streamed.status).toBe(200);
    expect(streamed.headers.get("content-type")).toContain("text/event-stream");
    const text = await streamed.text();
    expect(text).toContain("event: response.output_text.delta");
    expect(text).toContain("event: response.completed");
  });
});
