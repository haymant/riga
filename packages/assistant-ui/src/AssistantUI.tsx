import { Component, useEffect, useMemo, useRef, useState, type ReactNode, type SetStateAction } from "react";
import { createHttpTransport, formatBytes, reduceDownloadState } from "./http";
import type { DownloadState, LocalModelOverview, RigaEventEnvelope, RigaTransport, RigaTransportListeners } from "./protocol";
import {
  Bot,
  Check,
  ChevronDown,
  ChevronLeft,
  Square,
  Clock3,
  Code2,
  FolderOpen,
  GitBranch,
  Maximize2,
  Menu,
  MessageSquare,
  Minimize2,
  Paperclip,
  Pencil,
  Moon,
  Sun,
  Plus,
  Send,
  Settings2,
  ShieldCheck,
  TerminalSquare,
  X,
} from "lucide-react";
import "./styles.css";
import { Markdown } from "./Markdown";
import { DEFAULT_ASSISTANT_UI_OPTIONS, type AssistantUiOptions } from "./options";

// Sentinel for the composer's local-model entry. Distinct from any remote model
// id so selecting it is unambiguous.
const LOCAL_MODEL_VALUE = "__local_model__";

let idCounter = 0;

/**
 * A unique id that works outside a secure context.
 *
 * `crypto.randomUUID` is only defined over HTTPS or localhost. A phone opening
 * the dev server at `http://<lan-ip>:1420` gets `undefined`, and calling it
 * throws — which blanked the whole app the moment a message was sent.
 * `getRandomValues` is available in insecure contexts; the counter is the last
 * resort for a browser that has no WebCrypto at all.
 */
function newId(): string {
  const webCrypto = globalThis.crypto;
  // Both calls are feature-detected *and* guarded: a WebKit webview served from
  // the `tauri://` custom scheme can expose `randomUUID` yet throw "The
  // operation is insecure" because the origin is not a secure context. A throw
  // here previously blanked the app the moment a message was sent.
  try {
    if (typeof webCrypto?.randomUUID === "function") return webCrypto.randomUUID();
  } catch {
    // fall through to the counter
  }
  try {
    if (typeof webCrypto?.getRandomValues === "function") {
      const bytes = webCrypto.getRandomValues(new Uint8Array(16));
      bytes[6] = (bytes[6] & 0x0f) | 0x40;
      bytes[8] = (bytes[8] & 0x3f) | 0x80;
      const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    }
  } catch {
    // fall through to the counter
  }
  idCounter += 1;
  return `id-${Date.now().toString(36)}-${idCounter.toString(36)}`;
}


type Role = "user" | "assistant" | "system";
type Session = { id: string; title: string; meta: string; active?: boolean };
type TranscriptItem =
  | { id: string; role: Role; text: string; time: string }
  | { id: string; role: "reasoning"; text: string; time: string }
  | { id: string; role: "tool"; callId?: string; taskId?: string; name: string; command: string; status: "running" | "done" | "error"; output: string; time: string };
type TranscriptBlock = TranscriptItem | { id: string; role: "timeline"; items: Extract<TranscriptItem, { role: "tool" }>[] };
type CatalogItem = { id: string; kind: string; description: string; insert_text: string; requires_approval: boolean };
type SkillSummary = { name: string; description: string; path: string };
type AgentSummary = { name: string; aliases: string[]; purpose: string; read_only: boolean };
type FileCandidate = { name: string; path: string };
type McpServerSummary = { name: string; command: string; args?: string[]; tools: string[]; transport?: "stdio" | "http"; url?: string; api_key_configured?: boolean };
type ConnectorDraft = { name: string; transport: "stdio" | "http"; command: string; args: string; url: string; apiKey: string };
type CatalogLayer = "root" | "connectors";
type ReasoningEffort = "low" | "medium" | "high";
type Attachment = { name: string; path: string; size: number };
type PlanStep = { id?: string; label: string; description?: string };
type AgentPlanState = { title: string; steps: PlanStep[]; active_index: number };
type TodoStatus = "pending" | "active" | "done" | "failed" | "cancelled";
type TodoItemState = { id: string; text: string; description?: string; status: TodoStatus; reason?: string };
type TodoListState = { title?: string; revision?: number; items: TodoItemState[] };
type AgentTaskView = { id: string; agent: string; description: string; state: "running" | "done" | "failed"; result?: string };
type PendingApproval = { approvalId: string; tool: string; summary: string };

const initialSessions: Session[] = [
  { id: "riga", title: "RIGA desktop shell", meta: "Today · 14 messages", active: true },
  { id: "transport", title: "SSE transport contract", meta: "Yesterday · 8 messages" },
  { id: "recovery", title: "Recovery test plan", meta: "Mon · 21 messages" },
  { id: "ui", title: "Assistant surface audit", meta: "Sun · 6 messages" },
];

const initialTranscript: TranscriptItem[] = [
  { id: "m1", role: "user", text: "Wire the desktop shell to the durable session model and show the first approval step.", time: "10:42" },
  { id: "m2", role: "assistant", text: "I’ll connect the shell to the session boundary first, then pause before any workspace mutation so you can review the exact action.", time: "10:42" },
  { id: "t1", role: "tool", name: "session.inspect", command: "riga session inspect --id riga", status: "done", output: "Session riga · 1 active run · journal healthy", time: "10:43" },
  { id: "m3", role: "assistant", text: "The session is healthy. I’m ready to create the transport adapter, but this changes the workspace boundary and needs your approval.", time: "10:43" },
];

const sessionHistories: Record<string, TranscriptItem[]> = {
  riga: initialTranscript,
  transport: [
    { id: "transport-1", role: "user", text: "Compare SSE and WebSocket transport for the desktop shell.", time: "Yesterday" },
    { id: "transport-2", role: "assistant", text: "WebSocket is the active bidirectional control channel; SSE remains useful for one-way event delivery and reconnect replay.", time: "Yesterday" },
    { id: "transport-tool", role: "tool", name: "transport.inspect", command: "riga transport inspect --protocol websocket", status: "done", output: "WebSocket ready · protocol v1 · reconnect-safe", time: "Yesterday" },
  ],
  recovery: [
    { id: "recovery-1", role: "user", text: "Add corruption and out-of-order journal recovery tests.", time: "Mon" },
    { id: "recovery-2", role: "assistant", text: "The persistence boundary now fails closed on corrupted records and rejects out-of-order event sequences.", time: "Mon" },
    { id: "recovery-tool", role: "tool", name: "cargo.test", command: "cargo test -p riga-kernel persistence", status: "done", output: "Recovery tests passed", time: "Mon" },
  ],
  ui: [
    { id: "ui-1", role: "user", text: "Audit the assistant surface for responsive behavior.", time: "Sun" },
    { id: "ui-2", role: "assistant", text: "The desktop workspace uses a compact transcript layout, mobile navigation, and explicit provider settings.", time: "Sun" },
  ],
};

function loadLocal<T>(key: string, fallback: T): T {
  try {
    const stored = window.localStorage.getItem(key);
    return stored ? (JSON.parse(stored) as T) : fallback;
  } catch {
    return fallback;
  }
}

export type AssistantUIProps = AssistantUiOptions & {
  /** Supply a transport explicitly for desktop, tests, or another host runtime. */
  transport?: RigaTransport;
  /** Create a transport after the UI installs its event listeners. */
  transportFactory?: (listeners: RigaTransportListeners) => RigaTransport;
};

function AssistantUIInner({
  showSessionHistoryButton = DEFAULT_ASSISTANT_UI_OPTIONS.showSessionHistoryButton,
  fullWidth: initialFullWidth = DEFAULT_ASSISTANT_UI_OPTIONS.fullWidth,
  serverUrl,
  transport: suppliedTransport,
  transportFactory,
}: AssistantUIProps = {}) {
  const [sessions, setSessions] = useState<Session[]>(() => loadLocal("riga.sessions.v1", initialSessions));
  const [sessionTranscripts, setSessionTranscripts] = useState<Record<string, TranscriptItem[]>>(() => loadLocal("riga.transcripts.v1", sessionHistories));
  const [draft, setDraft] = useState("");
  const [composerHistory, setComposerHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [pendingApproval, setPendingApproval] = useState<PendingApproval | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [providerEndpoint, setProviderEndpoint] = useState("");
  const [providerApiKey, setProviderApiKey] = useState("");
  const [providerModel, setProviderModel] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>("low");
  // The settings tab the user is editing. Distinct from `providerKind`, which is
  // the backend runs actually use: they diverge as soon as the user opens
  // Settings while a local model is active.
  const [providerMode, setProviderMode] = useState<"remote" | "local">("remote");
  const [providerKind, setProviderKind] = useState<"remote" | "local">("remote");
  // Which remote API the form will call. Defaults to the OpenAI-compatible chat
  // contract; the Responses API is opt-in.
  const [providerApi, setProviderApi] = useState<"chat" | "responses">("chat");
  // Optional cheaper model for read-only subagents (explore/plan/review).
  const [subagentModel, setSubagentModel] = useState("");
  const [localModels, setLocalModels] = useState<LocalModelOverview | null>(null);
  const [downloads, setDownloads] = useState<Record<string, DownloadState>>({});
  const [modelBusy, setModelBusy] = useState<string | null>(null);
  const [modelError, setModelError] = useState<string | null>(null);
  // The agent's live plan and working list, driven by PlanUpdated/TodoUpdated
  // frames. Pinned above the composer so they stay visible as the transcript
  // scrolls, and durable across turns until the agent rewrites them.
  const [agentPlan, setAgentPlan] = useState<AgentPlanState | null>(null);
  const [agentTodos, setAgentTodos] = useState<TodoListState | null>(null);
  const [agentTasks, setAgentTasks] = useState<AgentTaskView[]>([]);
  const [theme, setTheme] = useState<"dark" | "light">(() => loadLocal("riga.theme.v1", "dark"));
  const [fullWidthEnabled, setFullWidthEnabled] = useState(initialFullWidth);
  const [transportStatus, setTransportStatus] = useState<"connecting" | "connected" | "closed" | "error">("connecting");
  const [catalogOpen, setCatalogOpen] = useState(false);
  const [manualCatalog, setManualCatalog] = useState(false);
  const [catalogLayer, setCatalogLayer] = useState<CatalogLayer>("root");
  const [catalogQuery, setCatalogQuery] = useState("");
  const [editingConnector, setEditingConnector] = useState<string | "new" | null>(null);
  const [connectorDraft, setConnectorDraft] = useState<ConnectorDraft>({ name: "", transport: "stdio", command: "", args: "", url: "", apiKey: "" });
  const [catalogItems, setCatalogItems] = useState<CatalogItem[]>([]);
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [workspaceFiles, setWorkspaceFiles] = useState<FileCandidate[]>([]);
  const [mcpServers, setMcpServers] = useState<McpServerSummary[]>([]);
  const transportRef = useRef<RigaTransport | null>(null);
  const activeRunIdRef = useRef<string | null>(null);
  const catalogRef = useRef<HTMLDivElement | null>(null);
  const transcriptRef = useRef<HTMLDivElement | null>(null);
  // Streamed text waiting to be painted. A model emits a token at a time and each
  // one would otherwise force a full markdown re-parse, so the deltas are
  // coalesced into a single animation frame.
  const pendingTextRef = useRef<{ runId: string; text: string } | null>(null);
  const textFrameRef = useRef<number | null>(null);

  const activeSession = useMemo(() => sessions.find((session) => session.active) ?? sessions[0], [sessions]);
  const transcript = sessionTranscripts[activeSession.id] ?? [];
  const taskTools = useMemo(() => {
    const map: Record<string, Extract<TranscriptItem, { role: "tool" }>[]> = {};
    for (const item of transcript) {
      if (item.role === "tool" && item.taskId) (map[item.taskId] ??= []).push(item);
    }
    return map;
  }, [transcript]);
  const setTranscript = (updater: SetStateAction<TranscriptItem[]>) => {
    setSessionTranscripts((current) => {
      const previous = current[activeSession.id] ?? [];
      const next = typeof updater === "function" ? updater(previous) : updater;
      return { ...current, [activeSession.id]: next };
    });
  };

  /**
   * Commit whatever streamed text is buffered into the transcript.
   *
   * Called from an animation frame so a run that emits deltas faster than the
   * display refresh costs one render per frame rather than one per token. The
   * markdown of a long reply is re-parsed on every render, so the difference is
   * visible once a reply grows past a few hundred tokens.
   */
  const flushStreamedText = () => {
    if (textFrameRef.current !== null) {
      window.cancelAnimationFrame(textFrameRef.current);
      textFrameRef.current = null;
    }
    const pending = pendingTextRef.current;
    if (!pending) return;
    pendingTextRef.current = null;
    setTranscript((current) => {
      const id = `stream-${pending.runId}`;
      // Matched anywhere, not just at the end: a run can emit text, call a tool,
      // then keep talking, and that continuation belongs to the same message.
      // Keying on the run id also stops a second run from appending to the
      // previous run's reply, which the old `startsWith("stream-")` test allowed.
      const at = current.findIndex((item) => item.id === id);
      if (at >= 0) {
        const item = current[at] as Extract<TranscriptItem, { role: "assistant" | "user" | "system" }>;
        const next = [...current];
        next[at] = { ...item, text: `${item.text}${pending.text}` };
        return next;
      }
      return [...current, { id, role: "assistant", text: pending.text, time: "now" }];
    });
  };

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    window.localStorage.setItem("riga.theme.v1", theme);
  }, [theme]);

  useEffect(() => {
    window.localStorage.setItem("riga.sessions.v1", JSON.stringify(sessions));
  }, [sessions]);

  useEffect(() => {
    window.localStorage.setItem("riga.transcripts.v1", JSON.stringify(sessionTranscripts));
  }, [sessionTranscripts]);

  useEffect(() => {
    const transcriptElement = transcriptRef.current;
    if (!transcriptElement) return;
    const frame = window.requestAnimationFrame(() => {
      transcriptElement.scrollTo({ top: transcriptElement.scrollHeight, behavior: "smooth" });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [transcript, isRunning]);

  const allConnectors = mcpServers;

  const triggerMatch = useMemo(() => {
    const match = draft.match(/(?:^|\s)([@/])([^\s]*)$/);
    return match ? { char: match[1], query: match[2] } : null;
  }, [draft]);
  const activeTrigger = manualCatalog ? null : triggerMatch;

  useEffect(() => {
    if (triggerMatch) {
      setCatalogOpen(true);
      setCatalogLayer("root");
      setCatalogQuery(triggerMatch.query);
    }
  }, [triggerMatch]);

  useEffect(() => {
    if (!catalogOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!catalogRef.current?.contains(event.target as Node)) setCatalogOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") { setCatalogOpen(false); setCatalogQuery(""); setCatalogLayer("root"); }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => { document.removeEventListener("pointerdown", onPointerDown); document.removeEventListener("keydown", onKeyDown); };
  }, [catalogOpen]);

  useEffect(() => {
    const listeners: RigaTransportListeners = {
      onStatus: setTransportStatus,
      onLocalModelEvent: (event) => {
        setDownloads((current) => reduceDownloadState(current, event));
        if (event.type === "download_finished" || event.type === "download_failed") {
          void transportRef.current?.listLocalModels().then(setLocalModels).catch(() => undefined);
        }
      },
      onProviderConfigured: (endpoint, model, effort, kind, api, subagent) => {
        // Endpoint and model are echoed even for a local provider so the remote
        // form survives a switch to local. `kind` decides what runs use.
        setProviderEndpoint(endpoint);
        setProviderModel(model);
        setReasoningEffort(effort);
        setProviderKind(kind);
        setProviderApi(api);
        setSubagentModel(subagent);
      },
      onError: (code, message) => {
        activeRunIdRef.current = null;
        setIsRunning(false);
        if (code === "provider_persist_failed") setToast(message);
        setTranscript((current) => [...current, { id: newId(), role: "system", text: `${code}: ${message}`, time: "now" }]);
      },
      onEvent: (envelope: RigaEventEnvelope) => {
        const event = envelope.event;
        if (typeof event === "object" && event !== null && "ToolCallStarted" in event) {
          const call = (event as { ToolCallStarted: { call: { call_id?: string; task_id?: string; name?: string; arguments?: unknown } } }).ToolCallStarted.call;
          // A tool call tagged with a task id belongs to a subagent, so the UI
          // nests it inside that task's card rather than the main timeline.
          setTranscript((current) => [...current, { id: envelope.event_id, role: "tool", callId: call.call_id, taskId: typeof call.task_id === "string" ? call.task_id : undefined, name: call.name ?? "tool", command: typeof call.arguments === "string" ? call.arguments : JSON.stringify(call.arguments ?? {}), status: "running", output: "Waiting for result…", time: "now" }]);
        } else if (typeof event === "object" && event !== null && "ToolOutputDelta" in event) {
          const output = (event as { ToolOutputDelta: { call_id?: string; delta?: string } }).ToolOutputDelta;
          setTranscript((current) => current.map((item) => item.role === "tool" && item.callId === output.call_id
            ? { ...item, output: `${item.output === "Waiting for result…" ? "" : item.output}${output.delta ?? ""}` }
            : item));
        } else if (typeof event === "object" && event !== null && "ToolResult" in event) {
          const result = (event as { ToolResult: { result: { call_id?: string; name?: string; output?: string; ok?: boolean } } }).ToolResult.result;
          setTranscript((current) => current.map((item) => item.role === "tool" && item.callId === result.call_id ? { ...item, status: result.ok ? "done" : "error", output: result.output ?? "" } : item));
        } else if (typeof event === "object" && event !== null && "TextDelta" in event) {
          const delta = (event as { TextDelta: { delta: string } }).TextDelta.delta;
          const runId = envelope.run_id;
          const pending = pendingTextRef.current;
          if (pending && pending.runId === runId) pending.text += delta;
          else pendingTextRef.current = { runId, text: delta };
          if (textFrameRef.current === null) {
            textFrameRef.current = window.requestAnimationFrame(flushStreamedText);
          }
        } else if (typeof event === "object" && event !== null && "ReasoningDelta" in event) {
          const delta = (event as { ReasoningDelta: { delta: string } }).ReasoningDelta.delta;
          // Reasoning streams as one collapsible block per run. Keying on the run
          // (not the event id) keeps it a single block even if a text or tool
          // frame lands between two reasoning deltas.
          setTranscript((current) => {
            const id = `reasoning-${envelope.run_id}`;
            const at = current.findIndex((item) => item.id === id);
            if (at >= 0) {
              const item = current[at] as Extract<TranscriptItem, { role: "reasoning" }>;
              const next = [...current];
              next[at] = { ...item, text: item.text + delta };
              return next;
            }
            return [...current, { id, role: "reasoning", text: delta, time: "now" }];
          });
        } else if (typeof event === "object" && event !== null && "RunCompleted" in event) {
          // Flush before marking the run done, otherwise the tail of the reply
          // would sit in the buffer until the next frame after the spinner stops.
          flushStreamedText();
          // The final output is authoritative: a streamed reply is replaced with
          // it so any post-hoc note (a verification or truncation marker) shows.
          const finalOutput = (event as { RunCompleted: { output: string } }).RunCompleted.output;
          const finalId = `stream-${envelope.run_id}`;
          setTranscript((current) => {
            const at = current.findIndex((item) => item.id === finalId);
            // No streamed message means the token frames never reached this
            // client — a run followed only after a reconnect, because the deltas
            // are not journaled. Create the message from the completed output
            // rather than dropping the whole reply.
            if (at < 0) {
              if (!finalOutput) return current;
              return [...current, { id: finalId, role: "assistant", text: finalOutput, time: "now" }];
            }
            const item = current[at] as Extract<TranscriptItem, { role: "assistant" | "user" | "system" }>;
            if (item.text === finalOutput) return current;
            const next = [...current];
            next[at] = { ...item, text: finalOutput };
            return next;
          });
          activeRunIdRef.current = null;
          setIsRunning(false);
        } else if (typeof event === "object" && event !== null && "RunFailed" in event) {
          flushStreamedText();
          activeRunIdRef.current = null;
          setIsRunning(false);
          setTranscript((current) => [...current, { id: envelope.event_id, role: "system", text: `Agent run failed: ${(event as { RunFailed: { message: string } }).RunFailed.message}`, time: "now" }]);
        } else if (typeof event === "object" && event !== null && "RunStarted" in event) {
          setAgentTasks([]);
          setIsRunning(true);
        } else if (typeof event === "object" && event !== null && "PlanUpdated" in event) {
          setAgentPlan((event as { PlanUpdated: { plan: AgentPlanState } }).PlanUpdated.plan);
        } else if (typeof event === "object" && event !== null && "TodoUpdated" in event) {
          setAgentTodos((event as { TodoUpdated: { list: TodoListState } }).TodoUpdated.list);
        } else if (typeof event === "object" && event !== null && "TaskStarted" in event) {
          const task = (event as { TaskStarted: { task: { id: string; agent: string; description: string } } }).TaskStarted.task;
          setAgentTasks((current) => [...current.filter((item) => item.id !== task.id), { id: task.id, agent: task.agent, description: task.description, state: "running" }]);
        } else if (typeof event === "object" && event !== null && "TaskCompleted" in event) {
          const done = (event as { TaskCompleted: { task_id: string; ok: boolean; result: string } }).TaskCompleted;
          setAgentTasks((current) => current.map((item) => item.id === done.task_id ? { ...item, state: done.ok ? "done" : "failed", result: done.result } : item));
        } else if (typeof event === "object" && event !== null && "ApprovalRequested" in event) {
          const request = (event as { ApprovalRequested: { approval_id: string; tool: string; summary: string } }).ApprovalRequested;
          setPendingApproval({ approvalId: request.approval_id, tool: request.tool, summary: request.summary });
        } else if (typeof event === "object" && event !== null && "ApprovalResolved" in event) {
          const resolved = (event as { ApprovalResolved: { approval_id: string } }).ApprovalResolved;
          setPendingApproval((current) => current && current.approvalId === resolved.approval_id ? null : current);
        }
      },
    };
    const client = suppliedTransport ?? transportFactory?.(listeners) ?? createHttpTransport(serverUrl ?? "", listeners);
    transportRef.current = client;
    void client.catalog().then((value) => {
      setCatalogItems((value.tools as CatalogItem[] | undefined) ?? []);
      setAgents((value.agents as AgentSummary[] | undefined) ?? []);
      setWorkspaceFiles((value.files as FileCandidate[] | undefined) ?? []);
      setSkills((value.skills as SkillSummary[] | undefined) ?? []);
      setMcpServers((value.mcp_servers as McpServerSummary[] | undefined) ?? []);
    }).catch(() => undefined);
    client.connect().catch(() => setTransportStatus("error"));
    return () => client.close();
  }, [serverUrl, suppliedTransport, transportFactory]);

  // A phone that was locked, a tab that was backgrounded, or a network that just
  // came back should reconnect at once rather than wait out a backoff timer that
  // may have been frozen while the page was suspended.
  useEffect(() => {
    const wake = () => transportRef.current?.wake();
    document.addEventListener("visibilitychange", wake);
    window.addEventListener("online", wake);
    window.addEventListener("focus", wake);
    return () => {
      document.removeEventListener("visibilitychange", wake);
      window.removeEventListener("online", wake);
      window.removeEventListener("focus", wake);
    };
  }, []);

  const refreshLocalModels = useMemo(() => async () => {
    try {
      setLocalModels(await transportRef.current?.listLocalModels() ?? null);
      setModelError(null);
    } catch (error) {
      setModelError(error instanceof Error ? error.message : "Local model manager is unavailable");
    }
  }, []);

  // The model manager is opened from Settings, so the catalog is fetched when
  // that panel first appears rather than on every render. The event subscription
  // is opened first and lives for the session: the server only broadcasts to
  // current subscribers, so a subscription opened at download time would miss
  // the opening progress frames.
  useEffect(() => {
    if (!settingsOpen || providerMode !== "local") return;
    void refreshLocalModels();
  }, [settingsOpen, providerMode, refreshLocalModels]);

  // Also read the local model list once the transport is up, so the composer can
  // offer the loaded model without the user having to open Settings first.
  useEffect(() => {
    if (transportStatus !== "connected") return;
    void refreshLocalModels();
  }, [transportStatus, refreshLocalModels]);

  async function runModelAction(label: string, action: () => Promise<void>) {
    setModelBusy(label);
    setModelError(null);
    try {
      await action();
      await refreshLocalModels();
    } catch (error) {
      setModelError(error instanceof Error ? error.message : `${label} failed`);
    } finally {
      setModelBusy(null);
    }
  }

  function selectSession(id: string) {
    setSessions((current) => current.map((session) => ({ ...session, active: session.id === id })));
    setHistoryOpen(false);
  }

  function createSession() {
    const next = { id: `session-${sessions.length + 1}`, title: "New coding session", meta: "Just now · 0 messages", active: true };
    setSessions((current) => [next, ...current.map((session) => ({ ...session, active: false }))]);
    setSessionTranscripts((current) => ({ ...current, [next.id]: [] }));
    setPendingApproval(null);
    setHistoryOpen(false);
    setToast("New session created");
  }

  function sendMessage() {
    const text = draft.trim();
    if (!text || isRunning) return;
    const attachmentContext = attachments.length > 0
      ? `\n\nAttached files are available to read from the workspace: ${attachments.map((attachment) => attachment.path).join(", ")}`
      : "";
    const prompt = `${text}${attachmentContext}`;
    setTranscript((current) => [...current, { id: newId(), role: "user", text: prompt, time: "now" }]);
    setComposerHistory((current) => [...current.filter((entry) => entry !== text), text]);
    setHistoryIndex(-1);
    setDraft("");
    setAttachments([]);
    setIsRunning(true);
    setPendingApproval(null);
    if (transportRef.current && transportStatus === "connected") {
      const runId = newId();
      activeRunIdRef.current = runId;
      void transportRef.current.startRun(runId, activeSession.id, prompt).catch(() => {
        activeRunIdRef.current = null;
        setTransportStatus("error");
        setIsRunning(false);
      });
      return;
    }
    setIsRunning(false);
    setTranscript((current) => [...current, { id: newId(), role: "system", text: "WebSocket is not connected. Configure a provider and wait for the connection before sending.", time: "now" }]);
  }

  function stopRun() {
    const runId = activeRunIdRef.current;
    activeRunIdRef.current = null;
    setIsRunning(false);
    if (runId) void transportRef.current?.cancelRun(runId);
    setToast("Run cancellation requested");
  }


  function navigateComposerHistory(direction: "up" | "down") {
    if (composerHistory.length === 0) return;
    if (direction === "up") {
      const nextIndex = historyIndex < 0 ? composerHistory.length - 1 : Math.max(0, historyIndex - 1);
      setHistoryIndex(nextIndex);
      setDraft(composerHistory[nextIndex]);
      return;
    }
    if (historyIndex < 0) return;
    if (historyIndex >= composerHistory.length - 1) {
      setHistoryIndex(-1);
      setDraft("");
      return;
    }
    const nextIndex = historyIndex + 1;
    setHistoryIndex(nextIndex);
    setDraft(composerHistory[nextIndex]);
  }

  async function uploadAttachments(files: FileList | null) {
    if (!files?.length) return;
    for (const file of Array.from(files)) {
      try {
        const uploaded = await transportRef.current?.uploadAttachment(file, activeSession.id);
        if (!uploaded) throw new Error("transport is not connected");
        setAttachments((current) => [...current, uploaded]);
        setToast(`${file.name} uploaded to this session's workspace`);
      } catch (error) {
        setToast(error instanceof Error ? error.message : "attachment upload failed");
      }
    }
  }

  function answerApproval(approved: boolean, option: "once" | "always") {
    const pending = pendingApproval;
    if (!pending) return;
    setPendingApproval(null);
    void transportRef.current?.respondToApproval(activeRunIdRef.current ?? "", pending.approvalId, approved, option);
  }

  function selectComposerModel(model: string) {
    if (model === LOCAL_MODEL_VALUE) {
      // Keep the stored endpoint and model so switching back to remote does not
      // lose them; the server ignores both while `kind` is local.
      setProviderKind("local");
      if (transportStatus === "connected") {
        void transportRef.current?.configureProvider(providerEndpoint.trim(), "", providerModel.trim(), reasoningEffort, "local", providerApi, subagentModel.trim());
      }
      return;
    }
    setProviderKind("remote");
    setProviderModel(model);
    if (model && providerEndpoint.trim() && transportStatus === "connected") {
      void transportRef.current?.configureProvider(providerEndpoint.trim(), "", model, reasoningEffort, "remote", providerApi, subagentModel.trim());
    }
  }

  function insertCatalog(text: string) {
    setDraft((current) => {
      if (activeTrigger) return current.replace(/(?:^|\s)([@/])([^\s]*)$/, (match) => `${match.startsWith(" ") ? " " : ""}${text}`);
      return `${current}${current ? "\n" : ""}${text}`;
    });
    setCatalogOpen(false);
    setManualCatalog(false);
    setCatalogQuery("");
    setCatalogLayer("root");
  }

  function openConnectorEditor(server?: McpServerSummary) {
    setEditingConnector(server?.name ?? "new");
    setConnectorDraft({ name: server?.name ?? "", transport: server?.transport ?? (server?.url ? "http" : "stdio"), command: server?.command === "unknown" ? "" : server?.command ?? "", args: server?.args?.join(" ") ?? "", url: server?.url ?? "", apiKey: "" });
  }

  async function saveConnector() {
    const name = connectorDraft.name.trim();
    if (!name || (connectorDraft.transport === "stdio" ? !connectorDraft.command.trim() : !connectorDraft.url.trim())) {
      setToast("Connector name and transport details are required");
      return;
    }
    try {
      const payload = await transportRef.current?.saveMcpRegistry({ name, transport: connectorDraft.transport, command: connectorDraft.transport === "stdio" ? connectorDraft.command.trim() : undefined, args: connectorDraft.args.trim() ? connectorDraft.args.trim().split(/\s+/) : [], url: connectorDraft.transport === "http" ? connectorDraft.url.trim() : undefined, api_key: connectorDraft.transport === "http" ? connectorDraft.apiKey.trim() || undefined : undefined });
      if (!payload) { setToast("Connector could not be persisted: transport is unavailable"); return; }
      const updated = payload;
      setMcpServers(updated);
      setEditingConnector(null);
      setToast(`Connector ${name} saved`);
    } catch {
      setToast("Connector could not be persisted: RIGA server is unavailable");
    }
  }

  const searchableItems = useMemo(() => {
    const query = catalogQuery.trim().toLowerCase();
    const tools = catalogItems.filter((item) => !query || `${item.id} ${item.description}`.toLowerCase().includes(query));
    const skillItems = skills.filter((skill) => !query || `${skill.name} ${skill.description}`.toLowerCase().includes(query));
    const connectorItems = allConnectors.filter((server) => !query || `${server.name} ${server.command} ${server.url ?? ""}`.toLowerCase().includes(query));
    const files = [...workspaceFiles, ...attachments.map((file) => ({ name: file.name, path: file.path }))]
      .filter((file, index, list) => list.findIndex((candidate) => candidate.path === file.path) === index)
      .filter((file) => !query || `${file.name} ${file.path}`.toLowerCase().includes(query));
    const agentItems = agents.filter((agent) => !query || `${agent.name} ${agent.aliases.join(" ")} ${agent.purpose}`.toLowerCase().includes(query));
    return { tools, skills: skillItems, connectors: connectorItems, files, agents: agentItems };
  }, [agents, allConnectors, attachments, catalogItems, catalogQuery, skills, workspaceFiles]);

  const menuTools = activeTrigger?.char === "@" ? [] : searchableItems.tools;
  const menuSkills = activeTrigger?.char === "@" ? [] : searchableItems.skills;
  const menuConnectors = activeTrigger?.char === "/" || activeTrigger?.char === "@" ? [] : searchableItems.connectors;

  async function saveProvider() {
    if (providerMode === "local") {
      // An in-process GGUF reads neither the endpoint nor the key, but they are
      // sent through rather than blanked: the store keeps one config, and
      // wiping the remote settings here would lose them on the way back.
      if (!localModels?.loaded) {
        setToast("Load a local model first");
        return;
      }
      await transportRef.current?.configureProvider(providerEndpoint.trim(), "", providerModel.trim(), reasoningEffort, "local", providerApi, subagentModel.trim());
      setSettingsOpen(false);
      setToast(`Local model ready: ${localModels.loaded}`);
      return;
    }
    if (!providerEndpoint.trim() || !providerModel.trim()) {
      setToast("Endpoint and model are required");
      return;
    }
    await transportRef.current?.configureProvider(providerEndpoint.trim(), providerApiKey, providerModel.trim(), reasoningEffort, "remote", providerApi, subagentModel.trim());
    setSettingsOpen(false);
    setToast(`Provider saved securely: ${providerModel.trim()}`);
  }

  return (
    <div className={`app-shell theme-${theme}`}>
      <main className="main-panel">
        <header className="app-header">
          <div className="app-brand"><div className="brand-mark"><Code2 size={15} strokeWidth={2.6} /></div><strong>RIGA</strong><span>@rigai/assistant-ui</span></div>
          <div className="app-header-actions"><span className={transportStatus === "connected" ? "connection-pill connected" : "connection-pill"}><span /> {transportStatus}</span><button className="icon-button" aria-label="Toggle theme" title="Toggle theme" onClick={() => setTheme((value) => value === "dark" ? "light" : "dark")}>{theme === "dark" ? <Sun size={15} /> : <Moon size={15} />}</button><button className={`icon-button ${fullWidthEnabled ? "selected" : ""}`} aria-label="Toggle full-width chat" title={fullWidthEnabled ? "Use centered chat width" : "Use full-width chat"} onClick={() => setFullWidthEnabled((value) => !value)}>{fullWidthEnabled ? <Minimize2 size={15} /> : <Maximize2 size={15} />}</button></div>
        </header>
        <section className={`content-column${fullWidthEnabled ? " full-width" : ""}`}>
          <header className="chat-header">
            <div className="chat-session-label"><div className="chat-session-icon"><MessageSquare size={14} /></div><div><span>Session</span><strong>{activeSession.title}</strong></div></div>
            <div className="chat-header-actions">
              {showSessionHistoryButton && <button className={`icon-button chat-header-button ${historyOpen ? "selected" : ""}`} aria-label="Open chat history" title="Open chat history" onClick={() => { setHistoryOpen((value) => !value); setSettingsOpen(false); }}><Menu size={16} /></button>}
              <button className={`icon-button chat-header-button ${settingsOpen ? "selected" : ""}`} aria-label="Open settings" title="Open settings" onClick={() => { setSettingsOpen((value) => !value); setHistoryOpen(false); }}><Settings2 size={16} /></button>
              <button className="icon-button chat-header-button" aria-label="New chat" title="New chat" onClick={createSession}><Plus size={16} /></button>
            </div>
            {historyOpen && <div className="chat-popover history-popover"><div className="chat-popover-header"><strong>Chat history</strong><button className="outline-button" onClick={createSession}><Plus size={13} /> New chat</button></div><nav className="compact-session-list" aria-label="Chat history">{sessions.map((session) => <button key={session.id} className={`compact-session-item ${session.active ? "active" : ""}`} onClick={() => selectSession(session.id)}><MessageSquare size={14} /><span><strong>{session.title}</strong><small>{session.meta}</small></span></button>)}</nav></div>}
            {settingsOpen && <section className="chat-popover settings-popover"><div className="settings-panel-header"><div><p className="eyebrow">RUNTIME / PROVIDER</p><h2>Connect your model.</h2><p>Endpoint and model restore after reload. The API key is sent over WebSocket and retained only in the server's encrypted store.</p></div><button className="icon-button" aria-label="Close settings" onClick={() => setSettingsOpen(false)}><X size={17} /></button></div><div className="provider-tabs"><button className={providerMode === "remote" ? "selected" : ""} onClick={() => setProviderMode("remote")}>OpenAI-compatible / OpenCode Go</button><button className={providerMode === "local" ? "selected" : ""} onClick={() => setProviderMode("local")}>Local GGUF model</button></div>{providerMode === "remote" ? <div className="provider-form"><label>API endpoint<input value={providerEndpoint} onChange={(event) => setProviderEndpoint(event.target.value)} placeholder="https://api.example.com/v1" /></label><label>API key <span>encrypted at rest</span><input type="password" value={providerApiKey} onChange={(event) => setProviderApiKey(event.target.value)} placeholder="sk-…" autoComplete="off" /></label><label>Model<input value={providerModel} onChange={(event) => setProviderModel(event.target.value)} placeholder="opencode-go / gpt-4o-mini" /></label><label>API<select value={providerApi} onChange={(event) => setProviderApi(event.target.value as "chat" | "responses")}><option value="chat">Chat completions (compatible)</option><option value="responses">Responses API (OpenAI)</option></select></label><label>Subagent model <span>optional</span><input value={subagentModel} onChange={(event) => setSubagentModel(event.target.value)} placeholder="cheap model for explore/plan" /></label><label>Reasoning effort<select value={reasoningEffort} onChange={(event) => setReasoningEffort(event.target.value as ReasoningEffort)}><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option></select></label><button className="approve-button settings-save" onClick={() => void saveProvider()}><Check size={15} /> Save securely</button></div> : <div className="local-model-panel"><div className="local-model-head"><div className="tool-symbol"><Bot size={17} /></div><div><strong>Local GGUF models</strong><p>Download a curated GGUF and run it in this process through llama.cpp. Runs on {localModels?.accelerator ?? "the local CPU"}{localModels?.accelerator?.includes("CPU") ? " — build with `--features cuda` for GPU offload." : "."}</p></div></div>{modelError && <p className="model-error">{modelError}</p>}<div className="model-section"><div className="model-section-title"><span>Downloaded</span><button className="outline-button" onClick={() => void refreshLocalModels()} disabled={modelBusy !== null}>Refresh</button></div>{!localModels && <p className="model-empty">Loading the model manager…</p>}{localModels?.installed.length === 0 && <p className="model-empty">No models yet. Download one below; it is verified against a pinned SHA-256 before use.</p>}{localModels?.installed.map((model) => { const state = downloads[model.id]; return <div className="model-row" key={model.path}><div className="model-row-main"><strong>{model.name}</strong><span>{formatBytes(model.size_bytes)} · {model.curated ? model.recommended_context ? `${model.recommended_context >= 1024 ? `${Math.round(model.recommended_context / 1024)}k` : model.recommended_context} ctx` : "curated GGUF" : "local GGUF"}</span></div><div className="model-row-actions">{state?.phase === "downloading" && <button className="outline-button" onClick={() => void runModelAction("cancel", async () => { await transportRef.current?.cancelDownload(model.id); })} disabled={modelBusy !== null}>{state.percent.toFixed(0)}% · Cancel</button>}{localModels.loaded === model.file_name ? <span className="model-loaded">Loaded</span> : <button className="approve-button" onClick={() => void runModelAction("load", async () => { await transportRef.current?.loadModel(model.path); })} disabled={modelBusy !== null}>{modelBusy === "load" ? "Loading…" : "Load"}</button>}</div>{state?.phase === "downloading" && <div className="model-progress"><span style={{ width: `${Math.max(2, state.percent)}%` }} /></div>}{state?.phase === "failed" && <p className="model-error">{state.message}</p>}</div>; })}{localModels && <div className="model-section-title"><span>Curated catalog</span></div>}{localModels?.catalog.map((model) => { const state = downloads[model.id]; const already = localModels.installed.some((installed) => installed.id === model.id); return <div className="model-row" key={model.id}><div className="model-row-main"><strong>{model.name}</strong><span>{formatBytes(model.size_bytes)} · {model.quant} · {Math.round(model.recommended_context / 1024)}k ctx · <a href={model.license_url} target="_blank" rel="noreferrer">license</a></span></div><div className="model-row-actions">{already ? <span className="model-installed-tag">Installed</span> : state?.phase === "downloading" ? <button className="outline-button" onClick={() => void runModelAction("cancel", async () => { await transportRef.current?.cancelDownload(model.id); })} disabled={modelBusy !== null}>{state.percent.toFixed(0)}% · Cancel</button> : state?.phase === "finished" ? <span className="model-installed-tag">Ready</span> : <button className="approve-button" onClick={() => void runModelAction("download", async () => { await transportRef.current?.downloadModel(model.id); })} disabled={modelBusy !== null}>{modelBusy === "download" ? "Starting…" : "Download"}</button>}</div>{state?.phase === "downloading" && <div className="model-progress"><span style={{ width: `${Math.max(2, state.percent)}%` }} /></div>}{state?.phase === "failed" && <p className="model-error">{state.message}</p>}</div>; })}</div><button className="approve-button settings-save" onClick={() => void saveProvider()} disabled={!localModels?.loaded}><Check size={15} /> Use this model</button>{!localModels?.loaded && <p className="model-hint">Load a model above to enable local runs.</p>}</div>}</section>}
          </header>
          <div ref={transcriptRef} className="transcript" aria-live="polite">
            {transcript.length === 0 && <div className="empty-state"><div className="empty-icon"><Bot size={26} /></div><h2>Start a coding run</h2><p>Describe the change, then review every tool action before it touches your workspace.</p></div>}
            {groupTranscript(transcript.filter((item) => !(item.role === "tool" && item.taskId))).map((block) => block.role === "timeline" ? <ToolTimeline key={block.id} items={block.items} /> : <TranscriptItemView key={block.id} item={block} />)}
            {isRunning && <ThinkingIndicator transcript={transcript} />}
          </div>

          {pendingApproval && <div className="approval-card"><div className="approval-icon"><ShieldCheck size={19} /></div><div className="approval-copy"><div className="approval-title"><strong>Approval required</strong><span>{pendingApproval.tool}</span></div><p>The agent wants to run <code>{pendingApproval.summary}</code>.</p></div><div className="approval-actions"><button className="deny-button" onClick={() => answerApproval(false, "once")}>Decline</button><button className="outline-button" onClick={() => answerApproval(true, "always")}>Always allow</button><button className="approve-button" onClick={() => answerApproval(true, "once")}><Check size={15} /> Allow once</button></div></div>}

          {(agentPlan || agentTodos || agentTasks.length > 0) && <div className="agent-work"><AgentPlanCard plan={agentPlan} /><AgentTodoList list={agentTodos} /><AgentTaskList tasks={agentTasks} toolRuns={taskTools} /></div>}
          <div className="composer-wrap">{attachments.length > 0 && <div className="composer-attachments">{attachments.map((attachment) => <span className="attachment-chip" key={attachment.path}><Paperclip size={12} /> {attachment.name}<button type="button" aria-label={`Remove ${attachment.name}`} onClick={() => setAttachments((current) => current.filter((item) => item.path !== attachment.path))}><X size={12} /></button></span>)}</div>}<div className="composer"><input ref={fileInputRef} className="file-input-hidden" type="file" multiple onChange={(event) => { void uploadAttachments(event.target.files); event.currentTarget.value = ""; }} /><button className="icon-button composer-icon" aria-label="Attach file" onClick={() => fileInputRef.current?.click()}><Paperclip size={17} /></button><div className="composer-model"><select aria-label="Configured model" className="composer-model-name" value={providerKind === "local" ? LOCAL_MODEL_VALUE : providerModel} onChange={(event) => selectComposerModel(event.target.value)}><option value="">Model</option>{localModels?.loaded && <option value={LOCAL_MODEL_VALUE}>Local · {localModels.loaded}</option>}{Array.from(new Set([providerModel, "gpt-5-nano", "gpt-5-mini", "gpt-5-codex"])).filter(Boolean).map((model) => <option key={model} value={model}>{model}</option>)}</select><select aria-label="Reasoning effort" value={reasoningEffort} onChange={(event) => setReasoningEffort(event.target.value as ReasoningEffort)}><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option></select></div><button className="icon-button composer-plus" aria-label="Insert tool, skill, or MCP" onPointerDown={(event) => event.stopPropagation()} onClick={() => { setCatalogOpen((value) => !value); setManualCatalog(true); setCatalogLayer("root"); setCatalogQuery(""); }}><Plus size={17} /></button><textarea value={draft} onChange={(event) => { setDraft(event.target.value); event.currentTarget.style.height = "auto"; event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 168)}px`; }} onKeyDown={(event) => { if (event.key === "ArrowUp" && !event.shiftKey && !event.altKey && !event.metaKey) { event.preventDefault(); navigateComposerHistory("up"); return; } if (event.key === "ArrowDown" && !event.shiftKey && !event.altKey && !event.metaKey) { event.preventDefault(); navigateComposerHistory("down"); return; } if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); sendMessage(); } }} placeholder="Ask RIGA to make a change…" rows={1} /><button className={`send-button ${isRunning ? "stop-ready" : draft.trim() ? "send-ready" : ""}`} aria-label={isRunning ? "Stop run" : "Send message"} onClick={isRunning ? stopRun : sendMessage}>{isRunning ? <Square size={14} fill="currentColor" /> : <Send size={16} />}</button></div>{catalogOpen && <div className="catalog-menu" ref={catalogRef} role="listbox">
              <div className="catalog-menu-header">{catalogLayer === "connectors" && <button className="catalog-back" aria-label="Back to insert menu" onClick={() => setCatalogLayer("root")}><ChevronLeft size={14} /></button>}<strong>{activeTrigger ? `${activeTrigger.char === "@" ? "Mention" : "Command"} suggestions` : catalogLayer === "root" ? "Insert into composer" : "Connectors"}</strong><button className="catalog-close" aria-label="Close insert menu" onClick={() => setCatalogOpen(false)}><X size={14} /></button></div>
              <input className="catalog-search" autoFocus={catalogOpen} value={catalogQuery} onChange={(event) => setCatalogQuery(event.target.value)} placeholder={activeTrigger ? `Filter ${activeTrigger.char === "@" ? "files or agents" : "tools and skills"}…` : "Search tools, skills, connectors…"} aria-label="Search composer insert menu" />
              {editingConnector ? <div className="connector-form"><label>Name<input value={connectorDraft.name} onChange={(event) => setConnectorDraft({ ...connectorDraft, name: event.target.value })} placeholder="my-server" /></label><label>Transport<select value={connectorDraft.transport} onChange={(event) => setConnectorDraft({ ...connectorDraft, transport: event.target.value as "stdio" | "http" })}><option value="stdio">stdio process</option><option value="http">HTTP stream</option></select></label>{connectorDraft.transport === "stdio" ? <><label>Command<input value={connectorDraft.command} onChange={(event) => setConnectorDraft({ ...connectorDraft, command: event.target.value })} placeholder="riga-server" /></label><label>Arguments<input value={connectorDraft.args} onChange={(event) => setConnectorDraft({ ...connectorDraft, args: event.target.value })} placeholder="mcp-health-stdio" /></label></> : <><label>HTTP stream URL<input value={connectorDraft.url} onChange={(event) => setConnectorDraft({ ...connectorDraft, url: event.target.value })} placeholder="http://127.0.0.1:8787/mcp/health" /></label><label>API key<input type="password" value={connectorDraft.apiKey} onChange={(event) => setConnectorDraft({ ...connectorDraft, apiKey: event.target.value })} placeholder="RIGA_MCP_HEALTH_API_KEY (optional)" autoComplete="off" /></label></>}<div className="connector-form-actions"><button className="outline-button" onClick={() => setEditingConnector(null)}>Cancel</button><button className="approve-button" onClick={saveConnector}><Check size={14} /> Save connector</button></div></div> : <>{catalogLayer === "root" && !activeTrigger && !catalogQuery && <><small>Connectors</small><button className="catalog-category" onClick={() => setCatalogLayer("connectors")}><span><FolderOpen size={14} /> MCP connectors</span><em>{allConnectors.length} registered <ChevronDown size={13} /></em></button><small>Built-in tools</small></>}{(catalogLayer === "connectors" || catalogQuery || activeTrigger?.char === "/" || activeTrigger?.char === "@") && <>{catalogLayer === "connectors" && <div className="catalog-inline-actions"><button className="catalog-category" onClick={() => openConnectorEditor()}><span><Plus size={14} /> Add connector</span><em>stdio or HTTP stream</em></button></div>}{menuConnectors.map((server) => <button className="catalog-item" key={`connector-${server.name}`} onClick={() => insertCatalog(`Use MCP server ${server.name}: `)}><span>mcp/{server.name}</span><em>{server.transport === "http" ? server.url : server.command}<button type="button" className="catalog-edit" aria-label={`Edit ${server.name}`} onClick={(event) => { event.stopPropagation(); openConnectorEditor(server); }}><Pencil size={12} /></button></em></button>)}</>}{(catalogLayer === "root" || catalogLayer === "connectors" || catalogQuery || activeTrigger) && <>{menuTools.length > 0 && <small>Built-in tools</small>}{menuTools.map((item) => <button className="catalog-item" key={item.id} onClick={() => insertCatalog(activeTrigger?.char === "@" ? `@${item.id} ` : activeTrigger?.char === "/" ? `/${item.id} ` : item.insert_text)}><span>{activeTrigger?.char === "/" ? `/${item.id}` : item.id}</span><em>{item.description}</em></button>)}{menuSkills.length > 0 && <small>Skills and agents</small>}{menuSkills.map((skill) => <button className="catalog-item" key={skill.name} onClick={() => insertCatalog(activeTrigger?.char === "@" ? `@${skill.name} ` : activeTrigger?.char === "/" ? `/${skill.name} ` : `Use the skill tool with name ${skill.name}: `)}><span>{activeTrigger?.char === "@" ? `@${skill.name}` : activeTrigger?.char === "/" ? `/${skill.name}` : `skill/${skill.name}`}</span><em>{skill.description}</em></button>)}{activeTrigger?.char === "@" && searchableItems.agents.length > 0 && <><small>Agents</small>{searchableItems.agents.map((agent) => <button className="catalog-item" key={`agent-${agent.name}`} onClick={() => insertCatalog(`@${agent.name} `)}><span>@{agent.name}</span><em>{agent.purpose}</em></button>)}</>}{activeTrigger?.char === "@" && searchableItems.files.length > 0 && <><small>Files</small>{searchableItems.files.map((file) => <button className="catalog-item" key={file.path} onClick={() => insertCatalog(`@${file.path} `)}><span>@{file.name}</span><em>{file.path}</em></button>)}</>}</>}</>}
              {catalogQuery && menuTools.length + menuSkills.length + menuConnectors.length + searchableItems.files.length + searchableItems.agents.length === 0 && <div className="catalog-empty">No matching tools, skills, or connectors.</div>}
            </div>}<div className="composer-footer"><span><kbd>Enter</kbd> send · <kbd>Shift Enter</kbd> newline · <kbd>↑↓</kbd> history</span><span>RIGA Kernel · local</span></div></div>
        </section>
      </main>
      {toast && <button className="toast" onClick={() => setToast(null)}><Check size={15} /> {toast}</button>}
    </div>
  );
}

function groupTranscript(items: TranscriptItem[]): TranscriptBlock[] {
  const blocks: TranscriptBlock[] = [];
  for (const item of items) {
    if (item.role === "tool") {
      const previous = blocks.at(-1);
      if (previous?.role === "timeline") {
        previous.items.push(item);
      } else {
        blocks.push({ id: `timeline-${item.id}`, role: "timeline", items: [item] });
      }
    } else {
      blocks.push(item);
    }
  }
  return blocks;
}

function ToolTimeline({ items }: { items: Extract<TranscriptItem, { role: "tool" }>[] }) {
  const running = items.some((item) => item.status === "running");
  const failed = items.some((item) => item.status === "error");
  return <details className="tool-timeline" aria-label="Agent tool timeline" open>
    <summary className="timeline-header"><div className="timeline-title"><Clock3 size={14} /><strong>{running ? "Working through tools" : failed ? "Tool run failed" : "Tool timeline"}</strong><span>{items.length} {items.length === 1 ? "step" : "steps"}</span></div><span className={`timeline-status ${running ? "running" : failed ? "error" : "complete"}`}>{running ? "Running" : failed ? "Needs attention" : "Completed"}</span></summary>
    <div className="timeline-rail">{items.map((item) => <ToolCallView key={item.id} item={item} />)}</div>
  </details>;
}

// Verb pair per tool, so a call reads as an action ("Read", "Ran") rather than
// its wire name. The settled verb shows once done, the active one while running.
const TOOL_VERBS: Record<string, [string, string]> = {
  read: ["Read", "Reading"],
  write: ["Wrote", "Writing"],
  glob: ["Listed", "Listing"],
  grep: ["Searched", "Searching"],
  bash: ["Ran", "Running"],
  shell: ["Ran", "Running"],
  web: ["Fetched", "Fetching"],
  task: ["Delegated", "Delegating"],
  skill: ["Ran skill", "Running skill"],
};

/** Parse a tool call's argument JSON, tolerating a non-object or invalid payload. */
function toolArguments(command: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(command) as unknown;
    return typeof parsed === "object" && parsed !== null ? (parsed as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

/** The primary argument, shown as the disclosure's chip (path, command, query…). */
function primaryToolArgument(args: Record<string, unknown>): string {
  for (const key of ["path", "command", "pattern", "query", "url", "agent", "name"]) {
    const value = args[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  for (const value of Object.values(args)) {
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return "";
}

// One tool invocation as the assistant-ui tool-call element: a single collapsed
// line (verb, primary-argument chip, status) that expands to the raw request and
// result. Never the `<tool_call>` payload the model emitted.
function ToolCallView({ item }: { item: Extract<TranscriptItem, { role: "tool" }> }) {
  const terminal = item.name === "bash" || item.name === "shell";
  const [verb, activeVerb] = TOOL_VERBS[item.name] ?? ["Called", "Calling"];
  const query = primaryToolArgument(toolArguments(item.command));
  const running = item.status === "running";
  const failed = item.status === "error";
  return <details className={`tool-card ${terminal ? "terminal-tool" : ""}`}>
    <summary className="tool-card-top">
      <div className="tool-symbol"><TerminalSquare size={15} /></div>
      <div><strong>{running ? activeVerb : verb}</strong><span className="tool-call-query" title={query || item.name}>{query || item.name}</span></div>
      <span className={`tool-status ${item.status}`}><span /> {running ? "Running" : failed ? "Failed" : "Completed"}</span>
    </summary>
    <div className="tool-output"><span className="tool-output-label">Request</span>{item.command || "{}"}</div>
    <div className={`tool-output ${terminal ? "terminal-output" : ""}`}><span className="tool-output-label">{failed ? "Tool error" : terminal ? "Terminal output" : "Tool result"}</span>{item.output || (running ? "Running…" : "")}</div>
  </details>;
}

// Reasoning as the assistant-ui "reasoning" element: the model's thinking,
// shown by default and still collapsible. Opening it is the point — a closed
// `<details>` collapses the whole block to its summary bar, which reads as a
// bare grey line rather than the reasoning the user asked to see.
function ReasoningView({ item }: { item: Extract<TranscriptItem, { role: "reasoning" }> }) {
  // A reasoning model may repeat the ` thinking` tag the template already opened;
  // it is the block delimiter, not reasoning text.
  const text = item.text.replace(/^\s*<\s*think\s*>\s*/i, "").trim();
  const words = text ? text.split(/\s+/).length : 0;
  return <details className="reasoning-block" open>
    <summary className="reasoning-summary"><Bot size={13} /><strong>Reasoning</strong><span>{words} {words === 1 ? "word" : "words"}</span></summary>
    <div className="reasoning-text">{text}</div>
  </details>;
}

// The live status line while a run is in flight: what the agent is doing now and
// for how long, per the assistant-ui "thinking indicator".
function ThinkingIndicator({ transcript }: { transcript: TranscriptItem[] }) {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => setSeconds((value) => value + 1), 1000);
    return () => window.clearInterval(id);
  }, []);
  const runningTool = [...transcript].reverse().find((item): item is Extract<TranscriptItem, { role: "tool" }> => item.role === "tool" && item.status === "running");
  const label = runningTool ? `${TOOL_VERBS[runningTool.name]?.[1] ?? "Running"} ${runningTool.name}` : "Thinking";
  return <div className="thinking-indicator"><span className="thinking-pulse" /><span className="thinking-label">{label}</span><span className="thinking-elapsed">{seconds}s</span></div>;
}

function TranscriptItemView({ item }: { item: TranscriptItem }) {
  if (item.role === "reasoning") return <ReasoningView item={item} />;
  if (item.role === "tool") return <ToolCallView item={item} />;
  // Only the assistant's prose is rendered as markdown. A user message is echoed
  // back verbatim on purpose: parsing it would reflow what was literally typed,
  // and the system lines are fixed UI strings with no markdown in them.
  const body = item.role === "assistant"
    ? <Markdown text={item.text} />
    : <p>{item.text}</p>;
  return <article className={`message-row ${item.role}`}><div className="message-avatar">{item.role === "assistant" ? <Bot size={15} /> : item.role === "system" ? <ShieldCheck size={15} /> : "AM"}</div><div className="message-body"><div className="message-meta"><strong>{item.role === "assistant" ? "RIGA" : item.role === "system" ? "System" : "You"}</strong><span>{item.time}</span></div>{body}</div></article>;
}

/** The plan the agent is working through, pinned above the composer. Mirrors
 * the assistant-ui AgentPlan element: a title, an "n of m" count, an
 * accessible progress bar, and one row per step with done/active/ahead state. */
function AgentPlanCard({ plan }: { plan: AgentPlanState | null }) {
  if (!plan) return null;
  const total = plan.steps.length;
  const active = Math.min(Math.max(plan.active_index, 0), total);
  const allDone = active >= total;
  return (
    <section className="agent-plan" aria-label="Agent plan">
      <div className="agent-card-head">
        <strong>{plan.title || "Plan"}</strong>
        <span>{active} of {total}</span>
      </div>
      <div className="agent-progress" role="progressbar" aria-valuemin={0} aria-valuemax={total} aria-valuenow={active}>
        <span style={{ width: `${total ? (active / total) * 100 : 0}%` }} />
      </div>
      <ul className="agent-steps">
        {plan.steps.map((step, index) => {
          const state = index < active || allDone ? "done" : index === active ? "active" : "ahead";
          return (
            <li key={step.id ?? `${index}-${step.label}`} className={`agent-step ${state}`}>
              <span className="agent-step-icon" aria-hidden>{state === "done" ? "✓" : state === "active" ? "•" : "○"}</span>
              <span>
                <span className="agent-step-label">{step.label}</span>
                {state === "active" && step.description ? <span className="agent-step-desc">{step.description}</span> : null}
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** The agent's live working list. A failed item counts toward the denominator
 * but not the numerator; a cancelled item counts toward neither, matching the
 * assistant-ui TodoList ratio. */
function AgentTodoList({ list }: { list: TodoListState | null }) {
  if (!list || list.items.length === 0) return null;
  const done = list.items.filter((item) => item.status === "done").length;
  const total = list.items.filter((item) => item.status !== "cancelled").length;
  return (
    <section className="agent-todos" aria-label="Agent todo list">
      <div className="agent-card-head">
        <strong>{list.title ?? "Todos"}</strong>
        <span>{done}/{total}{list.revision ? ` · rev ${list.revision}` : ""}</span>
      </div>
      <ul className="agent-steps">
        {list.items.map((item) => (
          <li key={item.id} className={`agent-step todo-${item.status}`}>
            <span className="agent-step-icon" aria-hidden>{todoIcon(item.status)}</span>
            <span>
              <span className="agent-step-label">{item.text}</span>
              {item.reason ? <span className="agent-step-desc">{item.reason}</span>
                : item.status === "active" && item.description ? <span className="agent-step-desc">{item.description}</span> : null}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function todoIcon(status: TodoStatus): string {
  switch (status) {
    case "done": return "✓";
    case "active": return "•";
    case "failed": return "✕";
    case "cancelled": return "–";
    default: return "○";
  }
}

/** The subagents dispatched during this run. Mirrors the assistant-ui
 * SubagentList: one row per worker with its state, plus the summary it
 * returned. */
function AgentTaskList({ tasks, toolRuns }: { tasks: AgentTaskView[]; toolRuns: Record<string, Extract<TranscriptItem, { role: "tool" }>[]> }) {
  if (tasks.length === 0) return null;
  const done = tasks.filter((task) => task.state !== "running").length;
  const failed = tasks.filter((task) => task.state === "failed").length;
  return (
    <section className="agent-tasks" aria-label="Agent subagents">
      <div className="agent-card-head">
        <strong>Subagents</strong>
        <span>{done}/{tasks.length}{failed ? ` · ${failed} failed` : ""}</span>
      </div>
      <ul className="agent-steps">
        {tasks.map((task) => {
          const runs = toolRuns[task.id] ?? [];
          const state = task.state === "running" ? "active" : task.state === "done" ? "done" : "failed";
          return (
            <li key={task.id} className={`agent-step todo-${state}`}>
              <span className="agent-step-icon" aria-hidden>{task.state === "running" ? "•" : task.state === "done" ? "✓" : "✕"}</span>
              <span className="agent-task-body">
                {runs.length === 0 ? (
                  <span className="agent-step-label">{task.agent} · {task.description}</span>
                ) : (
                  <details className="agent-task">
                    <summary className="agent-step-label">{task.agent} · {task.description} <span className="agent-step-count">{runs.length} step{runs.length === 1 ? "" : "s"}</span></summary>
                    <ul className="agent-task-tools">
                      {runs.map((run) => (
                        <li key={run.id} className={`agent-task-tool ${run.status}`}>
                          <code>{run.name}</code>
                          <span>{run.command.length > 80 ? `${run.command.slice(0, 80)}…` : run.command}</span>
                        </li>
                      ))}
                    </ul>
                  </details>
                )}
                {task.result ? <span className="agent-step-desc">{task.result.split("\n")[0]}</span> : null}
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/**
 * Catches a render/lifecycle error and shows it.
 *
 * Without this, any uncaught error unmounts the tree and leaves a blank page —
 * which is exactly what a phone showed with no way to open a console. Rendering
 * the message inline at least says what happened.
 */
class AssistantUIErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div style={{ minHeight: "100dvh", padding: 24, font: "13px/1.6 system-ui, sans-serif", color: "#e9edf5", background: "#11151d" }}>
          <h2 style={{ margin: "0 0 8px", fontSize: 16 }}>RIGA hit an error</h2>
          <p style={{ margin: "0 0 14px", opacity: 0.8 }}>{String(this.state.error.message || this.state.error)}</p>
          <button type="button" onClick={() => window.location.reload()}>Reload</button>
        </div>
      );
    }
    return this.props.children;
  }
}

export function AssistantUI(props: AssistantUIProps = {}) {
  return (
    <AssistantUIErrorBoundary>
      <AssistantUIInner {...props} />
    </AssistantUIErrorBoundary>
  );
}

export default AssistantUI;
