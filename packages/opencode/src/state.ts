import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, join } from "node:path";
import { randomUUID } from "node:crypto";
import { env, platform } from "node:process";
import type { OpenAIResponse } from "./protocol.js";

type StateDocument = {
  version: 2;
  responses: Record<string, OpenAIResponse>;
};

function emptyState(): StateDocument {
  return { version: 2, responses: {} };
}

export function defaultStatePath(): string {
  if (env.RIGA_OPENCODE_STATE_PATH?.trim()) return env.RIGA_OPENCODE_STATE_PATH;
  if (env.RIGA_OPENCODE_DATA_DIR?.trim()) return join(env.RIGA_OPENCODE_DATA_DIR, "responses.json");
  const root = platform === "win32"
    ? env.LOCALAPPDATA ?? join(homedir(), "AppData", "Local")
    : platform === "darwin"
      ? join(homedir(), "Library", "Application Support")
      : env.XDG_DATA_HOME ?? join(homedir(), ".local", "share");
  return join(root, "riga", "opencode", "responses.json");
}

export class ResponseStateStore {
  private state = emptyState();
  private loaded: Promise<void> | null = null;
  private saveQueue: Promise<void> = Promise.resolve();

  constructor(readonly filePath = defaultStatePath()) {}

  async getResponse(responseId: string): Promise<OpenAIResponse | undefined> {
    await this.load();
    return this.state.responses[responseId];
  }

  async recordResponse(response: OpenAIResponse): Promise<void> {
    await this.load();
    this.state.responses[response.id] = response;
    await this.save();
  }

  private async load(): Promise<void> {
    if (!this.loaded) {
      this.loaded = (async () => {
        try {
          const parsed: unknown = JSON.parse(await readFile(this.filePath, "utf8"));
          if (!parsed || typeof parsed !== "object") {
            throw new Error("Unsupported OpenCode response state format.");
          }
          const value = parsed as { version?: unknown; responses?: Record<string, OpenAIResponse>; responseObjects?: Record<string, OpenAIResponse> };
          if (value.version === 1) {
            // Preserve GET response retrieval for data written by the previous
            // adapter while dropping its response-id-to-session indirection.
            this.state = { version: 2, responses: value.responseObjects ?? {} };
          } else if (value.version === 2) {
            this.state = { version: 2, responses: value.responses ?? {} };
          } else {
            throw new Error("Unsupported OpenCode response state format.");
          }
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
          throw error;
        }
      })();
    }
    await this.loaded;
  }

  private async save(): Promise<void> {
    const snapshot = JSON.stringify(this.state, null, 2);
    const write = this.saveQueue.then(async () => {
      await mkdir(dirname(this.filePath), { recursive: true });
      const temporary = join(dirname(this.filePath), `.${basename(this.filePath)}.${randomUUID()}.tmp`);
      await writeFile(temporary, snapshot, { mode: 0o600 });
      await rename(temporary, this.filePath);
    });
    this.saveQueue = write.catch(() => undefined);
    await write;
  }
}
