// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AssistantUI } from "./AssistantUI";
import type { RigaEventEnvelope, RigaTransport, RigaTransportListeners } from "./protocol";

const emptyCatalog = {
  tools: [],
  agents: [],
  files: [],
  skills: [],
  mcp_servers: [],
};

const emptyModels = {
  accelerator: "CPU",
  catalog: [],
  installed: [],
  loaded: null,
};

function envelope(event: RigaEventEnvelope["event"], sequence: number, runId = "run-1"): RigaEventEnvelope {
  return {
    protocol_version: 1,
    event_id: `event-${sequence}`,
    session_id: "riga",
    run_id: runId,
    sequence,
    timestamp: "now",
    event,
  };
}

function createTransport() {
  let listeners: RigaTransportListeners | undefined;
  const transport: RigaTransport = {
    connect: async () => { listeners?.onStatus?.("connected"); },
    wake: () => undefined,
    close: () => undefined,
    health: async () => ({ protocol_version: 1, adapter: "test" }),
    catalog: async () => emptyCatalog,
    listSessions: async () => [],
    createSession: async (request) => ({ id: "riga", title: request.title, workspace: request.workspace, created_at: "now", updated_at: "now" }),
    sessionHistory: async () => [],
    renameSession: async (sessionId, title) => ({ id: sessionId, title, workspace: ".", created_at: "now", updated_at: "now" }),
    configureProvider: async () => undefined,
    startRun: async () => undefined,
    resumeRun: async () => undefined,
    cancelRun: async () => undefined,
    listActiveRuns: async () => [],
    respondToApproval: async () => undefined,
    listMcpRegistry: async () => [],
    saveMcpRegistry: async () => [],
    uploadAttachment: async (file) => ({ name: file.name, path: `tmp/${file.name}`, size: file.size }),
    listLocalModels: async () => emptyModels,
    subscribeLocalModels: () => () => undefined,
    downloadModel: async () => undefined,
    cancelDownload: async () => undefined,
    loadModel: async () => undefined,
    unloadModel: async () => undefined,
  };
  return {
    transport,
    factory: (nextListeners: RigaTransportListeners) => {
      listeners = nextListeners;
      return transport;
    },
    emit: (event: RigaEventEnvelope["event"], sequence: number, runId?: string) => {
      act(() => listeners?.onEvent?.(envelope(event, sequence, runId)));
    },
  };
}

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

beforeEach(() => {
  HTMLElement.prototype.scrollTo = vi.fn();
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
});

describe("AssistantUI subagent task card", () => {
  it("renders a running subagent, nested tool steps, and its completed result", async () => {
    const testTransport = createTransport();
    render(<AssistantUI transportFactory={testTransport.factory} />);

    await waitFor(() => expect(screen.getByText("connected")).toBeInTheDocument());

    testTransport.emit({
      RunStarted: {},
    }, 1);
    testTransport.emit({
      TaskStarted: {
        task: {
          id: "task-1",
          parent_id: null,
          agent: "explore",
          description: "Inspect the task implementation",
          model: "test-model",
          state: "running",
          started_at: "now",
        },
      },
    }, 2);
    testTransport.emit({
      ToolCallStarted: {
        call: {
          call_id: "call-1",
          task_id: "task-1",
          name: "read",
          arguments: { path: "crates/riga-kernel/src/task.rs" },
        },
      },
    }, 3);
    testTransport.emit({ ToolOutputDelta: { call_id: "call-1", delta: "TaskTree implementation" } }, 4);
    testTransport.emit({
      ToolResult: {
        result: { call_id: "call-1", name: "read", output: "TaskTree implementation", ok: true },
      },
    }, 5);

    fireEvent.click(screen.getByRole("button", { name: "Toggle Run Deck" }));
    expect(screen.getByRole("region", { name: "Agent subagents" })).toBeInTheDocument();
    expect(screen.getByText("Subagents")).toBeInTheDocument();
    expect(screen.getAllByText("0/1").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText(/explore · Inspect the task implementation/)).toBeInTheDocument();
    expect(screen.getByText("read")).toBeInTheDocument();
    expect(screen.getByText(/crates\/riga-kernel\/src\/task\.rs/)).toBeInTheDocument();

    testTransport.emit({
      TaskCompleted: {
        task_id: "task-1",
        ok: true,
        result: "Found the TaskTree lifecycle and limits.",
      },
    }, 6);

    expect(screen.getAllByText("1/1").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("Found the TaskTree lifecycle and limits.")).toBeInTheDocument();
    expect(screen.getByText("✓")).toBeInTheDocument();
  });

  it("marks a failed subagent and shows the failure summary", async () => {
    const testTransport = createTransport();
    render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("New session")).toBeInTheDocument());

    testTransport.emit({
      TaskStarted: {
        task: {
          id: "task-failed",
          parent_id: null,
          agent: "review",
          description: "Review the change",
          model: "test-model",
          state: "running",
          started_at: "now",
        },
      },
    }, 1);
    testTransport.emit({
      TaskCompleted: {
        task_id: "task-failed",
        ok: false,
        result: "review subagent failed: provider unavailable",
      },
    }, 2);

    fireEvent.click(screen.getByRole("button", { name: "Toggle Run Deck" }));
    expect(screen.getByText("1/1 · 1 failed")).toBeInTheDocument();
    expect(screen.getByText("review · Review the change")).toBeInTheDocument();
    expect(screen.getByText("review subagent failed: provider unavailable")).toBeInTheDocument();
    expect(screen.getByText("✕")).toBeInTheDocument();
  });

  it("collapses persistently and supports execution scope and node drill-down", async () => {
    const testTransport = createTransport();
    render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("New session")).toBeInTheDocument());

    testTransport.emit({ RunStarted: {} }, 1);
    testTransport.emit({
      GraphUpdated: {
        graph: {
          title: "Build change",
          nodes: [
            { id: "runtime", profile: "explore", description: "Inspect runtime", prompt: "inspect runtime", depends_on: [] },
            { id: "review", profile: "review", description: "Review findings", prompt: "review findings", depends_on: ["runtime"] },
          ],
        },
      },
    }, 2);

    fireEvent.click(screen.getByRole("button", { name: "Toggle Run Deck" }));
    expect(screen.getByText("Execution graph")).toBeInTheDocument();
    expect(screen.getByText("0/2")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Execution" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "Evidence" })).not.toBeDisabled();
    expect(screen.getByRole("tab", { name: "Knowledge" })).not.toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: /explore.*Inspect runtime.*ready/ }));
    expect(screen.getByText("runtime")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Session › Run ›" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Toggle Run Deck" }));
    expect(screen.queryByText("Execution graph")).not.toBeInTheDocument();
    expect(window.localStorage.getItem("riga.run-deck.riga.collapsed")).toBe("true");

    fireEvent.click(screen.getByRole("button", { name: "Toggle Run Deck" }));
    expect(screen.getByText("Execution graph")).toBeInTheDocument();

    expect(screen.getByRole("combobox", { name: "Run scope" })).toHaveValue("run-1");
    expect(screen.getByRole("option", { name: "run-1 · active" })).toBeInTheDocument();
  });

  it("resets the multiline composer after sending", async () => {
    const testTransport = createTransport();
    render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("New session")).toBeInTheDocument());

    const composer = screen.getByRole("textbox", { name: "" }) as HTMLTextAreaElement;
    Object.defineProperty(composer, "scrollHeight", { configurable: true, value: 120 });
    fireEvent.change(composer, { target: { value: "line one\nline two" } });
    expect(composer.style.height).toBe("120px");

    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    expect(composer.style.height).toBe("auto");
  });

  it("does not follow streamed text after the user scrolls up", async () => {
    const testTransport = createTransport();
    const { container } = render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("New session")).toBeInTheDocument());

    const transcript = container.querySelector(".transcript") as HTMLDivElement;
    Object.defineProperties(transcript, {
      scrollHeight: { configurable: true, value: 1000 },
      clientHeight: { configurable: true, value: 500 },
      scrollTop: { configurable: true, writable: true, value: 200 },
    });
    fireEvent.scroll(transcript);
    vi.mocked(HTMLElement.prototype.scrollTo).mockClear();

    testTransport.emit({ TextDelta: { delta: "streaming while reading history" } }, 1);
    expect(HTMLElement.prototype.scrollTo).not.toHaveBeenCalled();

    transcript.scrollTop = 500;
    fireEvent.scroll(transcript);
    testTransport.emit({ TextDelta: { delta: "follow from bottom" } }, 2);
    await waitFor(() => expect(HTMLElement.prototype.scrollTo).toHaveBeenCalled());
  });

  it("names a new session from its first assistant response", async () => {
    const testTransport = createTransport();
    const rename = vi.spyOn(testTransport.transport, "renameSession");
    const startRun = vi.spyOn(testTransport.transport, "startRun");
    render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("New session")).toBeInTheDocument());

    fireEvent.change(screen.getByRole("textbox"), { target: { value: "Explain the runtime" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    const runId = startRun.mock.calls[0]?.[0];
    expect(runId).toBeTruthy();
    testTransport.emit({ RunStarted: {} }, 1, runId);
    testTransport.emit({ TextDelta: { delta: "The runtime coordinates session execution." } }, 2, runId);
    testTransport.emit({ RunCompleted: { output: "The runtime coordinates session execution." } }, 3, runId);

    await waitFor(() => expect(rename).toHaveBeenCalledWith("riga", "The runtime coordinates session execution."));
    await waitFor(() => expect(screen.getAllByText("The runtime coordinates session execution.").length).toBeGreaterThan(0));
  });

  it("lets the user rename a session from the GUI", async () => {
    const testTransport = createTransport();
    const rename = vi.spyOn(testTransport.transport, "renameSession");
    render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("New session")).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Rename session New session" }));
    const editor = screen.getByRole("textbox", { name: "Session name" });
    fireEvent.change(editor, { target: { value: "Design notes" } });
    fireEvent.keyDown(editor, { key: "Enter" });

    await waitFor(() => expect(rename).toHaveBeenCalledWith("riga", "Design notes"));
    await waitFor(() => expect(screen.getAllByText("Design notes").length).toBeGreaterThan(0));
  });
});
