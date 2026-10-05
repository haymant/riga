import { afterEach, describe, expect, it, vi } from "vitest";
import { LocalModelClient, formatBytes, reduceDownloadState } from "./local-models";
import type { DownloadState, LocalModelEvent } from "./local-models";

const progress = (percent: number): LocalModelEvent => ({
  type: "download_progress",
  model_id: "m",
  downloaded_bytes: Math.round(percent),
  total_bytes: 100,
  percent,
});

describe("reduceDownloadState", () => {
  it("tracks progress for one model without disturbing another", () => {
    const first = reduceDownloadState({}, progress(40));
    expect(first.m).toEqual({
      phase: "downloading",
      percent: 40,
      downloaded_bytes: 40,
      total_bytes: 100,
    });
    const second = reduceDownloadState(
      { ...first, other: { phase: "downloading", percent: 10, downloaded_bytes: 10, total_bytes: 100 } },
      progress(80),
    );
    expect(second.m?.percent).toBe(80);
    expect(second.other?.percent).toBe(10);
  });

  it("marks a finished download complete and keeps the last byte counts", () => {
    const state = reduceDownloadState(reduceDownloadState({}, progress(97)), {
      type: "download_finished",
      model_id: "m",
      path: "/models/m.gguf",
    });
    expect(state.m).toEqual({
      phase: "finished",
      percent: 100,
      // Carried over from the last progress frame, not reset to zero.
      downloaded_bytes: 97,
      total_bytes: 100,
    });
  });

  it("completes a download that finished without a prior progress frame", () => {
    // A fast host can finish before the first progress event is rendered; the
    // UI must still show 100% rather than NaN.
    const state = reduceDownloadState({}, { type: "download_finished", model_id: "m", path: "/m.gguf" });
    expect(state.m?.percent).toBe(100);
    expect(state.m?.downloaded_bytes).toBe(0);
  });

  it("keeps the failure message and the progress reached before it failed", () => {
    const state = reduceDownloadState(reduceDownloadState({}, progress(63)), {
      type: "download_failed",
      model_id: "m",
      message: "Model SHA-256 verification failed; the incomplete file was discarded",
    });
    expect(state.m).toEqual({
      phase: "failed",
      percent: 63,
      downloaded_bytes: 63,
      total_bytes: 100,
      message: "Model SHA-256 verification failed; the incomplete file was discarded",
    });
  });

  it("reports a failure that arrives with no prior progress", () => {
    const state = reduceDownloadState({}, { type: "download_failed", model_id: "m", message: "Download cancelled" });
    expect(state.m?.percent).toBe(0);
    expect(state.m?.message).toBe("Download cancelled");
  });

  it("ignores token events, which belong to inference rather than downloads", () => {
    const before: Record<string, DownloadState> = { m: { phase: "downloading", percent: 5, downloaded_bytes: 5, total_bytes: 100 } };
    const after = reduceDownloadState(before, { type: "token", generation_id: "g", delta: "hi" });
    expect(after).toBe(before);
  });
});

describe("formatBytes", () => {
  it("scales to a readable unit", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(986_048_768)).toBe("940.4 MB");
    expect(formatBytes(2_019_377_696)).toBe("1.9 GB");
  });

  it("never renders NaN or a negative size", () => {
    // A catalog entry with an unknown size must not print "NaN undefined".
    expect(formatBytes(Number.NaN)).toBe("0 B");
    expect(formatBytes(-1)).toBe("0 B");
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe("0 B");
  });
});

/**
 * Build a fetch double that records its calls and replies with `body`/`status`.
 *
 * The parameter list is declared rather than left to inference so the recorded
 * calls can be read back as `(url, init)` tuples in the assertions below.
 */
function stubFetch(body: unknown, status = 200) {
  return vi.fn(async (_url: string, _init?: RequestInit) => new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  }));
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("LocalModelClient", () => {
  it("reads the overview from the server", async () => {
    const overview = { accelerator: "CPU (OpenMP)", catalog: [], installed: [], loaded: null };
    const fetchMock = stubFetch(overview);
    vi.stubGlobal("fetch", fetchMock);

    await expect(new LocalModelClient("").overview()).resolves.toEqual(overview);
    expect(fetchMock).toHaveBeenCalledWith("/local-models", undefined);
  });

  it("posts each action with the payload the server expects", async () => {
    const fetchMock = stubFetch({ ok: true });
    vi.stubGlobal("fetch", fetchMock);
    const client = new LocalModelClient("http://127.0.0.1:8787");

    await client.download("phi-4-mini-instruct");
    await client.cancelDownload("phi-4-mini-instruct");
    await client.load("/models/phi-4-mini-instruct-Q4_K_M.gguf");
    await client.unload();

    const sent = fetchMock.mock.calls.map((call) => JSON.parse(String(call[1]?.body)));
    expect(sent).toEqual([
      { action: "download", model_id: "phi-4-mini-instruct" },
      { action: "cancel", model_id: "phi-4-mini-instruct" },
      { action: "load", path: "/models/phi-4-mini-instruct-Q4_K_M.gguf" },
      { action: "unload" },
    ]);
    expect(fetchMock.mock.calls[0]?.[0]).toBe("http://127.0.0.1:8787/local-models");
    expect(fetchMock.mock.calls[0]?.[1]?.method).toBe("POST");
  });

  it("surfaces the server's error message rather than a bare status code", async () => {
    // The user needs to know the difference between "unknown model id" and
    // "already downloading"; a 400 with no text is useless on screen.
    vi.stubGlobal("fetch", stubFetch({ error: "Unknown curated model: not-a-model" }, 400));

    await expect(new LocalModelClient("").download("not-a-model"))
      .rejects.toThrow("Unknown curated model: not-a-model");
  });

  it("falls back to the status code when the failure carries no JSON body", async () => {
    // A proxy or a 502 from Vite can answer with HTML; that must not mask the
    // failure as a JSON parse error.
    vi.stubGlobal("fetch", vi.fn(async () => new Response("<html>bad gateway</html>", { status: 502 })));

    await expect(new LocalModelClient("").load("m.gguf"))
      .rejects.toThrow("RIGA local model request failed with 502");
  });
});
