// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AssistantUI } from "./AssistantUI";
import type { RigaTransport, RigaTransportListeners } from "./protocol";

const catalogWithSkills = {
  tools: [{ id: "read", kind: "tool", description: "Read a file", insert_text: "read ", requires_approval: false }],
  agents: [{ name: "explore", aliases: ["scout"], purpose: "Read-only reconnaissance", read_only: true }],
  files: [{ name: "main.rs", path: "src/main.rs" }],
  skills: [
    { name: "rundeck", description: "Orchestrate subagents", path: "skills/rundeck/SKILL.md" },
    { name: "code-review", description: "Review code", path: "skills/code-review/SKILL.md" },
  ],
  mcp_servers: [],
};

function createTransport() {
  let listeners: RigaTransportListeners | undefined;
  const transport: RigaTransport = {
    connect: async () => { listeners?.onStatus?.("connected"); },
    wake: () => undefined,
    close: () => undefined,
    health: async () => ({ protocol_version: 1, adapter: "test" }),
    catalog: async () => catalogWithSkills,
    listSessions: async () => [],
    createSession: async (request) => ({ id: "session-1", title: request.title, workspace: request.workspace, created_at: "now", updated_at: "now" }),
    sessionHistory: async () => [],
    renameSession: async (sessionId, title) => ({ id: sessionId, title, workspace: ".", created_at: "now", updated_at: "now" }),
    configureProvider: async () => undefined,
    remoteModels: async () => [],
    providerProfiles: async () => [],
    saveProviderProfile: async (input) => ({ ...input, id: input.id ?? "profile-new", api_key_configured: Boolean(input.api_key) }),
    deleteProviderProfile: async () => undefined,
    startRun: async () => undefined,
    resumeRun: async () => undefined,
    cancelRun: async () => undefined,
    listActiveRuns: async () => [],
    respondToApproval: async () => undefined,
    listMcpRegistry: async () => [],
    saveMcpRegistry: async () => [],
    uploadAttachment: async (file) => ({ name: file.name, path: `tmp/${file.name}`, size: file.size }),
    listLocalModels: async () => ({ accelerator: "CPU", catalog: [], installed: [], loaded: null }),
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

describe("composer + menu", () => {
  it("lists skills from the catalog", async () => {
    const testTransport = createTransport();
    render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("connected")).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Insert tool, skill, or MCP" }));

    expect(await screen.findByText("skill/rundeck")).toBeInTheDocument();
    expect(screen.getByText("skill/code-review")).toBeInTheDocument();
  });

  it("shows native slash commands and TUI-compatible file and subagent mentions", async () => {
    const testTransport = createTransport();
    render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("connected")).toBeInTheDocument());
    const composer = screen.getByPlaceholderText("Ask RIGA to make a change…");

    fireEvent.change(composer, { target: { value: "/mod" } });
    expect(await screen.findByText("/model")).toBeInTheDocument();
    expect(screen.getByText("/models")).toBeInTheDocument();

    fireEvent.change(composer, { target: { value: "@expl" } });
    expect(await screen.findByText("@explore")).toBeInTheDocument();
    fireEvent.change(composer, { target: { value: "@src" } });
    expect(await screen.findByText("@main.rs")).toBeInTheDocument();
    fireEvent.change(composer, { target: { value: "literal@expl" } });
    expect(screen.queryByText("@explore")).not.toBeInTheDocument();
  });

  it("uses TUI slash labels and tool-provided insertion text for dynamic tools", async () => {
    const testTransport = createTransport();
    render(<AssistantUI transportFactory={testTransport.factory} />);
    await waitFor(() => expect(screen.getByText("connected")).toBeInTheDocument());
    const composer = screen.getByPlaceholderText("Ask RIGA to make a change…");
    fireEvent.change(composer, { target: { value: "/tools/rea" } });

    const candidate = await screen.findByText("/tools/read");
    fireEvent.click(candidate.closest("button")!);
    expect(composer).toHaveValue("read ");
  });
});
