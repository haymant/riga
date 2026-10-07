/** Client for the local GGUF model manager exposed by `riga-server`. */

import type { DownloadState, LocalModelEvent, LocalModelOverview } from "../protocol";

export type { DownloadState, LocalModelEvent, LocalModelOverview } from "../protocol";

export class LocalModelClient {
  constructor(private readonly baseUrl: string) {}

  async overview(): Promise<LocalModelOverview> {
    return this.request("/local-models");
  }

  async download(modelId: string): Promise<void> {
    await this.action({ action: "download", model_id: modelId });
  }

  async cancelDownload(modelId: string): Promise<void> {
    await this.action({ action: "cancel", model_id: modelId });
  }

  async load(path: string): Promise<void> {
    await this.action({ action: "load", path });
  }

  async unload(): Promise<void> {
    await this.action({ action: "unload" });
  }

  /**
   * Subscribe to download progress and terminal state.
   *
   * Uses `EventSource`, so callers must be in a browser. Returns a function that
   * closes the stream. Call this *before* requesting a download: the server only
   * broadcasts to live subscribers, so subscribing afterwards can miss the first
   * events and leave the UI stuck at 0%.
   */
  subscribe(onEvent: (event: LocalModelEvent) => void): () => void {
    const source = new EventSource(`${this.baseUrl}/local-models/downloads/events`);
    source.addEventListener("local-model", (message) => {
      try {
        onEvent(JSON.parse((message as MessageEvent<string>).data) as LocalModelEvent);
      } catch {
        // A malformed frame must not kill the subscription; the next frame is
        // still useful and the UI re-syncs from `overview()`.
      }
    });
    return () => source.close();
  }

  private async action(body: Record<string, string>): Promise<void> {
    await this.request("/local-models", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  }

  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, init);
    if (!response.ok) {
      // Surface the server's message rather than a bare status code: load
      // failures here are things a user can act on (corrupt file, no memory).
      const detail = await response
        .json()
        .then((value: { error?: string }) => value.error)
        .catch(() => undefined);
      throw new Error(detail ?? `RIGA local model request failed with ${response.status}`);
    }
    return (await response.json()) as T;
  }
}

/** Fold an event stream into per-model state for the picker. */
export function reduceDownloadState(
  state: Record<string, DownloadState>,
  event: LocalModelEvent,
): Record<string, DownloadState> {
  if (event.type === "download_progress") {
    return {
      ...state,
      [event.model_id]: {
        phase: "downloading",
        percent: event.percent,
        downloaded_bytes: event.downloaded_bytes,
        total_bytes: event.total_bytes,
      },
    };
  }
  if (event.type === "download_finished") {
    // The finished frame carries no byte counts, so keep whatever the last
    // progress event reported and simply mark it complete.
    const previous = state[event.model_id];
    return {
      ...state,
      [event.model_id]: {
        phase: "finished",
        percent: 100,
        downloaded_bytes: previous?.downloaded_bytes ?? 0,
        total_bytes: previous?.total_bytes ?? 0,
      },
    };
  }
  if (event.type === "download_failed") {
    const previous = state[event.model_id];
    return {
      ...state,
      [event.model_id]: {
        phase: "failed",
        percent: previous?.percent ?? 0,
        downloaded_bytes: previous?.downloaded_bytes ?? 0,
        total_bytes: previous?.total_bytes ?? 0,
        message: event.message,
      },
    };
  }
  return state;
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const exponent = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  const value = bytes / 1024 ** exponent;
  // One decimal below 10 keeps "1.9 GB" readable; whole numbers above it.
  return `${exponent === 0 ? value : value.toFixed(1)} ${units[exponent]}`;
}
