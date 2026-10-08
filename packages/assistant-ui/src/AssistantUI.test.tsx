// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
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

function envelope(event: RigaEventEnvelope["event"], sequence: number): RigaEventEnvelope {
  return {
    protocol_version: 1,
    event_id: `event-${sequence}`,
    session_id: "riga",
    run_id: "run-1",
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
    createSession: async (request) => ({ id: "session-1", title: request.title, workspace: request.workspace, created_at: "now", updated_at: "now" }),
    configureProvider: async () => undefined,
    startRun: async () => undefined,
    resumeRun: async () => undefined,
    cancelRun: async () => undefined,
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
    emit: (event: RigaEventEnvelope["event"], sequence: number) => {
      act(() => listeners?.onEvent?.(envelope(event, sequence)));
    },
  };
}

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

beforeEach(() => {
  HTMLElement.prototype.scrollTo = vi.fn();
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

    expect(screen.getByRole("region", { name: "Agent subagents" })).toBeInTheDocument();
    expect(screen.getByText("Subagents")).toBeInTheDocument();
    expect(screen.getByText("0/1")).toBeInTheDocument();
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

    expect(screen.getByText("1/1")).toBeInTheDocument();
    expect(screen.getByText("Found the TaskTree lifecycle and limits.")).toBeInTheDocument();
    expect(screen.getByText("✓")).toBeInTheDocument();
  });

  it("marks a failed subagent and shows the failure summary", async () => {
    const testTransport = createTransport();
    render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("connected")).toBeInTheDocument());

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

    expect(screen.getByText("1/1 · 1 failed")).toBeInTheDocument();
    expect(screen.getByText("review · Review the change")).toBeInTheDocument();
    expect(screen.getByText("review subagent failed: provider unavailable")).toBeInTheDocument();
    expect(screen.getByText("✕")).toBeInTheDocument();
  });
});
