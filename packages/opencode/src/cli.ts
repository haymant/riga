#!/usr/bin/env node
import { createOpencode, createOpencodeClient } from "@opencode-ai/sdk";
import { createServer as createHttpServer } from "node:http";
import { env, exit, on } from "node:process";
import { createResponseApiServer } from "./server.js";
import type { OpenCodeClient } from "./protocol.js";

function positivePort(value: string | undefined, fallback: number): number {
  if (!value) return fallback;
  const port = Number(value);
  if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error(`Invalid port: ${value}`);
  return port;
}

function isLoopbackHost(host: string): boolean {
  return ["localhost", "127.0.0.1", "::1", "[::1]"].includes(host.toLowerCase());
}

async function main(): Promise<void> {
  const apiHost = env.RIGA_OPENCODE_HOST ?? "127.0.0.1";
  const apiPort = positivePort(env.RIGA_OPENCODE_PORT, 8788);
  if (!isLoopbackHost(apiHost) && !env.RIGA_OPENCODE_API_KEY) {
    throw new Error("RIGA_OPENCODE_API_KEY is required when binding the Responses API outside localhost/loopback.");
  }
  const opencodeHost = env.OPENCODE_HOST ?? "127.0.0.1";
  const opencodePort = positivePort(env.OPENCODE_PORT, 4096);

  const owned = env.OPENCODE_BASE_URL
    ? null
    : await createOpencode({ hostname: opencodeHost, port: opencodePort });
  const client = (owned?.client ?? createOpencodeClient({ baseUrl: env.OPENCODE_BASE_URL! })) as unknown as OpenCodeClient;
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
  if (owned) console.log(`OpenCode server running at ${owned.server.url}`);
  if (env.RIGA_OPENCODE_API_KEY) console.log("Bearer-token authentication is enabled.");

  let closing = false;
  const close = () => {
    if (closing) return;
    closing = true;
    server.close((error) => {
      owned?.server.close();
      if (error) {
        console.error(error);
        exit(1);
      }
      exit(0);
    });
    server.closeAllConnections();
  };
  on("SIGINT", close);
  on("SIGTERM", close);
}

void main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : error);
  exit(1);
});
