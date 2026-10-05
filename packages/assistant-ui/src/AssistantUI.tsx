import { useEffect, useMemo, useRef, useState, type SetStateAction } from "react";
import { RigaWebSocketClient, LocalModelClient, formatBytes, reduceDownloadState, type DownloadState, type LocalModelOverview, type RigaEventEnvelope } from "@haymant/transport-http";
import {
  Bot,
  Check,
  ChevronDown,
  ChevronLeft,
  CircleStop,
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

// An empty base URL makes every request resolve against the page origin, which
// is how the host already talks to the kernel (`/catalog`, `/attachments`) and
// what lets the container's dev proxy forward it unchanged.
const localModelClient = new LocalModelClient("");

// Sentinel for the composer's local-model entry. Distinct from any remote model
// id so selecting it is unambiguous.
const LOCAL_MODEL_VALUE = "__local_model__";

type Role = "user" | "assistant" | "system";
type Session = { id: string; title: string; meta: string; active?: boolean };
type TranscriptItem =
  | { id: string; role: Role; text: string; time: string }
  | { id: string; role: "tool"; callId?: string; name: string; command: string; status: "running" | "done" | "error"; output: string; time: string };
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

export type AssistantUIProps = AssistantUiOptions;

export function AssistantUI({
  showSessionHistoryButton = DEFAULT_ASSISTANT_UI_OPTIONS.showSessionHistoryButton,
  fullWidth: initialFullWidth = DEFAULT_ASSISTANT_UI_OPTIONS.fullWidth,
}: AssistantUIProps = {}) {
  const [sessions, setSessions] = useState<Session[]>(() => loadLocal("riga.sessions.v1", initialSessions));
  const [sessionTranscripts, setSessionTranscripts] = useState<Record<string, TranscriptItem[]>>(() => loadLocal("riga.transcripts.v1", sessionHistories));
  const [draft, setDraft] = useState("");
  const [composerHistory, setComposerHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [approval, setApproval] = useState(false);
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
  const [localModels, setLocalModels] = useState<LocalModelOverview | null>(null);
  const [downloads, setDownloads] = useState<Record<string, DownloadState>>({});
  const [modelBusy, setModelBusy] = useState<string | null>(null);
  const [modelError, setModelError] = useState<string | null>(null);
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
  const transportRef = useRef<RigaWebSocketClient | null>(null);
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
    void fetch("/catalog").then((response) => response.json()).then((value: { tools?: CatalogItem[]; agents?: AgentSummary[]; files?: FileCandidate[]; skills?: SkillSummary[]; mcp_servers?: McpServerSummary[] }) => {
      setCatalogItems(value.tools ?? []);
      setAgents(value.agents ?? []);
      setWorkspaceFiles(value.files ?? []);
      setSkills(value.skills ?? []);
      setMcpServers(value.mcp_servers ?? []);
    }).catch(() => undefined);
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const client = new RigaWebSocketClient({
      url: `${protocol}//${window.location.host}/ws`,
      onStatus: setTransportStatus,
      onProviderConfigured: (endpoint, model, effort, kind, api) => {
        // Endpoint and model are echoed even for a local provider so the remote
        // form survives a switch to local. `kind` decides what runs use.
        setProviderEndpoint(endpoint);
        setProviderModel(model);
        setReasoningEffort(effort);
        setProviderKind(kind);
        setProviderApi(api);
      },
      onError: (code, message) => {
        activeRunIdRef.current = null;
        setIsRunning(false);
        if (code === "provider_persist_failed") setToast(message);
        setTranscript((current) => [...current, { id: crypto.randomUUID(), role: "system", text: `${code}: ${message}`, time: "now" }]);
      },
      onEvent: (envelope: RigaEventEnvelope) => {
        const event = envelope.event;
        if (typeof event === "object" && event !== null && "ToolCallStarted" in event) {
          const call = (event as { ToolCallStarted: { call: { call_id?: string; name?: string; arguments?: unknown } } }).ToolCallStarted.call;
          setTranscript((current) => [...current, { id: envelope.event_id, role: "tool", callId: call.call_id, name: call.name ?? "tool", command: typeof call.arguments === "string" ? call.arguments : JSON.stringify(call.arguments ?? {}), status: "running", output: "Waiting for result…", time: "now" }]);
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
        } else if (typeof event === "object" && event !== null && "RunCompleted" in event) {
          // Flush before marking the run done, otherwise the tail of the reply
          // would sit in the buffer until the next frame after the spinner stops.
          flushStreamedText();
          activeRunIdRef.current = null;
          setIsRunning(false);
        } else if (typeof event === "object" && event !== null && "RunFailed" in event) {
          flushStreamedText();
          activeRunIdRef.current = null;
          setIsRunning(false);
          setTranscript((current) => [...current, { id: envelope.event_id, role: "system", text: `Agent run failed: ${(event as { RunFailed: { message: string } }).RunFailed.message}`, time: "now" }]);
        } else if (typeof event === "object" && event !== null && "RunStarted" in event) {
          setIsRunning(true);
        }
      },
    });
    transportRef.current = client;
    client.connect().catch(() => setTransportStatus("error"));
    return () => client.close();
  }, []);

  const refreshLocalModels = useMemo(() => async () => {
    try {
      setLocalModels(await localModelClient.overview());
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

  useEffect(() => {
    const unsubscribe = localModelClient.subscribe((event) => {
      setDownloads((current) => reduceDownloadState(current, event));
      // A finished or failed transfer changes what is on disk, so re-read the
      // authoritative list rather than guessing from the event.
      if (event.type === "download_finished" || event.type === "download_failed") {
        void refreshLocalModels();
      }
    });
    return unsubscribe;
  }, [refreshLocalModels]);

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
    setApproval(false);
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
    setTranscript((current) => [...current, { id: crypto.randomUUID(), role: "user", text: prompt, time: "now" }]);
    setComposerHistory((current) => [...current.filter((entry) => entry !== text), text]);
    setHistoryIndex(-1);
    setDraft("");
    setAttachments([]);
    setIsRunning(true);
    setApproval(false);
    if (transportRef.current && transportStatus === "connected") {
      const runId = crypto.randomUUID();
      activeRunIdRef.current = runId;
      void transportRef.current.startRun(runId, activeSession.id, prompt).catch(() => {
        activeRunIdRef.current = null;
        setTransportStatus("error");
        setIsRunning(false);
      });
      return;
    }
    setIsRunning(false);
    setTranscript((current) => [...current, { id: crypto.randomUUID(), role: "system", text: "WebSocket is not connected. Configure a provider and wait for the connection before sending.", time: "now" }]);
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
      const form = new FormData();
      form.append("file", file, file.name);
      try {
        const response = await fetch("/attachments", { method: "POST", body: form });
        const uploaded = (await response.json()) as Attachment & { error?: string };
        if (!response.ok) throw new Error(uploaded.error ?? "attachment upload failed");
        setAttachments((current) => [...current, uploaded]);
        setToast(`${file.name} uploaded to the temporary workspace`);
      } catch (error) {
        setToast(error instanceof Error ? error.message : "attachment upload failed");
      }
    }
  }

  function approve() {
    setApproval(false);
    void transportRef.current?.respondToApproval("active-run", "transport-write", true);
    setToast("Approval recorded; no workspace mutation is attached to this run");
  }

  function deny() {
    setApproval(false);
    void transportRef.current?.respondToApproval("active-run", "transport-write", false);
    setToast("Approval declined; no workspace mutation was made");
  }

  function selectComposerModel(model: string) {
    if (model === LOCAL_MODEL_VALUE) {
      // Keep the stored endpoint and model so switching back to remote does not
      // lose them; the server ignores both while `kind` is local.
      setProviderKind("local");
      if (transportStatus === "connected") {
        void transportRef.current?.configureProvider(providerEndpoint.trim(), "", providerModel.trim(), reasoningEffort, "local", providerApi);
      }
      return;
    }
    setProviderKind("remote");
    setProviderModel(model);
    if (model && providerEndpoint.trim() && transportStatus === "connected") {
      void transportRef.current?.configureProvider(providerEndpoint.trim(), "", model, reasoningEffort, "remote", providerApi);
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
      const response = await fetch("/mcp/registry", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name, transport: connectorDraft.transport, command: connectorDraft.transport === "stdio" ? connectorDraft.command.trim() : undefined, args: connectorDraft.args.trim() ? connectorDraft.args.trim().split(/\s+/) : [], url: connectorDraft.transport === "http" ? connectorDraft.url.trim() : undefined, api_key: connectorDraft.transport === "http" ? connectorDraft.apiKey.trim() || undefined : undefined }) });
      const payload = await response.json().catch(() => ({})) as McpServerSummary & { error?: string };
      if (!response.ok) { setToast(payload.error ?? `Connector could not be persisted (${response.status})`); return; }
      const updated = [...allConnectors.filter((server) => server.name !== name), payload];
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
      await transportRef.current?.configureProvider(providerEndpoint.trim(), "", providerModel.trim(), reasoningEffort, "local", providerApi);
      setSettingsOpen(false);
      setToast(`Local model ready: ${localModels.loaded}`);
      return;
    }
    if (!providerEndpoint.trim() || !providerModel.trim()) {
      setToast("Endpoint and model are required");
      return;
    }
    await transportRef.current?.configureProvider(providerEndpoint.trim(), providerApiKey, providerModel.trim(), reasoningEffort, "remote", providerApi);
    setSettingsOpen(false);
    setToast(`Provider saved securely: ${providerModel.trim()}`);
  }

  return (
    <div className={`app-shell theme-${theme}`}>
      <main className="main-panel">
        <header className="app-header">
          <div className="app-brand"><div className="brand-mark"><Code2 size={15} strokeWidth={2.6} /></div><strong>RIGA</strong><span>@haymant/assistant-ui</span></div>
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
            {settingsOpen && <section className="chat-popover settings-popover"><div className="settings-panel-header"><div><p className="eyebrow">RUNTIME / PROVIDER</p><h2>Connect your model.</h2><p>Endpoint and model restore after reload. The API key is sent over WebSocket and retained only in the server's encrypted store.</p></div><button className="icon-button" aria-label="Close settings" onClick={() => setSettingsOpen(false)}><X size={17} /></button></div><div className="provider-tabs"><button className={providerMode === "remote" ? "selected" : ""} onClick={() => setProviderMode("remote")}>OpenAI-compatible / OpenCode Go</button><button className={providerMode === "local" ? "selected" : ""} onClick={() => setProviderMode("local")}>Local GGUF model</button></div>{providerMode === "remote" ? <div className="provider-form"><label>API endpoint<input value={providerEndpoint} onChange={(event) => setProviderEndpoint(event.target.value)} placeholder="https://api.example.com/v1" /></label><label>API key <span>encrypted at rest</span><input type="password" value={providerApiKey} onChange={(event) => setProviderApiKey(event.target.value)} placeholder="sk-…" autoComplete="off" /></label><label>Model<input value={providerModel} onChange={(event) => setProviderModel(event.target.value)} placeholder="opencode-go / gpt-4o-mini" /></label><label>API<select value={providerApi} onChange={(event) => setProviderApi(event.target.value as "chat" | "responses")}><option value="chat">Chat completions (compatible)</option><option value="responses">Responses API (OpenAI)</option></select></label><label>Reasoning effort<select value={reasoningEffort} onChange={(event) => setReasoningEffort(event.target.value as ReasoningEffort)}><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option></select></label><button className="approve-button settings-save" onClick={() => void saveProvider()}><Check size={15} /> Save securely</button></div> : <div className="local-model-panel"><div className="local-model-head"><div className="tool-symbol"><Bot size={17} /></div><div><strong>Local GGUF models</strong><p>Download a curated GGUF and run it in this process through llama.cpp. Runs on {localModels?.accelerator ?? "the local CPU"}{localModels?.accelerator?.includes("CPU") ? " — build with `--features cuda` for GPU offload." : "."}</p></div></div>{modelError && <p className="model-error">{modelError}</p>}<div className="model-section"><div className="model-section-title"><span>Downloaded</span><button className="outline-button" onClick={() => void refreshLocalModels()} disabled={modelBusy !== null}>Refresh</button></div>{!localModels && <p className="model-empty">Loading the model manager…</p>}{localModels?.installed.length === 0 && <p className="model-empty">No models yet. Download one below; it is verified against a pinned SHA-256 before use.</p>}{localModels?.installed.map((model) => { const state = downloads[model.id]; return <div className="model-row" key={model.path}><div className="model-row-main"><strong>{model.name}</strong><span>{formatBytes(model.size_bytes)} · {model.curated ? model.recommended_context ? `${model.recommended_context >= 1024 ? `${Math.round(model.recommended_context / 1024)}k` : model.recommended_context} ctx` : "curated GGUF" : "local GGUF"}</span></div><div className="model-row-actions">{state?.phase === "downloading" && <button className="outline-button" onClick={() => void runModelAction("cancel", () => localModelClient.cancelDownload(model.id))} disabled={modelBusy !== null}>{state.percent.toFixed(0)}% · Cancel</button>}{localModels.loaded === model.file_name ? <span className="model-loaded">Loaded</span> : <button className="approve-button" onClick={() => void runModelAction("load", () => localModelClient.load(model.path))} disabled={modelBusy !== null}>{modelBusy === "load" ? "Loading…" : "Load"}</button>}</div>{state?.phase === "downloading" && <div className="model-progress"><span style={{ width: `${Math.max(2, state.percent)}%` }} /></div>}{state?.phase === "failed" && <p className="model-error">{state.message}</p>}</div>; })}{localModels && <div className="model-section-title"><span>Curated catalog</span></div>}{localModels?.catalog.map((model) => { const state = downloads[model.id]; const already = localModels.installed.some((installed) => installed.id === model.id); return <div className="model-row" key={model.id}><div className="model-row-main"><strong>{model.name}</strong><span>{formatBytes(model.size_bytes)} · {model.quant} · {Math.round(model.recommended_context / 1024)}k ctx · <a href={model.license_url} target="_blank" rel="noreferrer">license</a></span></div><div className="model-row-actions">{already ? <span className="model-installed-tag">Installed</span> : state?.phase === "downloading" ? <button className="outline-button" onClick={() => void runModelAction("cancel", () => localModelClient.cancelDownload(model.id))} disabled={modelBusy !== null}>{state.percent.toFixed(0)}% · Cancel</button> : state?.phase === "finished" ? <span className="model-installed-tag">Ready</span> : <button className="approve-button" onClick={() => void runModelAction("download", () => localModelClient.download(model.id))} disabled={modelBusy !== null}>{modelBusy === "download" ? "Starting…" : "Download"}</button>}</div>{state?.phase === "downloading" && <div className="model-progress"><span style={{ width: `${Math.max(2, state.percent)}%` }} /></div>}{state?.phase === "failed" && <p className="model-error">{state.message}</p>}</div>; })}</div><button className="approve-button settings-save" onClick={() => void saveProvider()} disabled={!localModels?.loaded}><Check size={15} /> Use this model</button>{!localModels?.loaded && <p className="model-hint">Load a model above to enable local runs.</p>}</div>}</section>}
          </header>
          <div ref={transcriptRef} className="transcript" aria-live="polite">
            {transcript.length === 0 && <div className="empty-state"><div className="empty-icon"><Bot size={26} /></div><h2>Start a coding run</h2><p>Describe the change, then review every tool action before it touches your workspace.</p></div>}
            {groupTranscript(transcript).map((block) => block.role === "timeline" ? <ToolTimeline key={block.id} items={block.items} /> : <TranscriptItemView key={block.id} item={block} />)}
            {isRunning && <div className="typing-row"><div className="assistant-badge"><Bot size={15} /></div><div className="typing-bubble"><span /><span /><span /></div><small>RIGA is thinking</small></div>}
          </div>

          {approval && <div className="approval-card"><div className="approval-icon"><ShieldCheck size={19} /></div><div className="approval-copy"><div className="approval-title"><strong>Approval required</strong><span>workspace mutation</span></div><p>Allow RIGA to write the transport adapter boundary in <code>crates/</code> and update the event journal contract?</p><div className="approval-details"><span><FolderOpen size={13} /> 3 files</span><span><GitBranch size={13} /> reversible change</span><span><Clock3 size={13} /> requested now</span></div></div><div className="approval-actions"><button className="deny-button" onClick={deny}>Decline</button><button className="approve-button" onClick={approve}><Check size={15} /> Approve</button></div></div>}

          <div className="composer-wrap">{attachments.length > 0 && <div className="composer-attachments">{attachments.map((attachment) => <span className="attachment-chip" key={attachment.path}><Paperclip size={12} /> {attachment.name}<button type="button" aria-label={`Remove ${attachment.name}`} onClick={() => setAttachments((current) => current.filter((item) => item.path !== attachment.path))}><X size={12} /></button></span>)}</div>}<div className="composer"><input ref={fileInputRef} className="file-input-hidden" type="file" multiple onChange={(event) => { void uploadAttachments(event.target.files); event.currentTarget.value = ""; }} /><button className="icon-button composer-icon" aria-label="Attach file" onClick={() => fileInputRef.current?.click()}><Paperclip size={17} /></button><div className="composer-model"><select aria-label="Configured model" value={providerKind === "local" ? LOCAL_MODEL_VALUE : providerModel} onChange={(event) => selectComposerModel(event.target.value)}><option value="">Model</option>{localModels?.loaded && <option value={LOCAL_MODEL_VALUE}>Local · {localModels.loaded}</option>}{Array.from(new Set([providerModel, "gpt-5-nano", "gpt-5-mini", "gpt-5-codex"])).filter(Boolean).map((model) => <option key={model} value={model}>{model}</option>)}</select><select aria-label="Reasoning effort" value={reasoningEffort} onChange={(event) => setReasoningEffort(event.target.value as ReasoningEffort)}><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option></select></div><button className="icon-button composer-plus" aria-label="Insert tool, skill, or MCP" onPointerDown={(event) => event.stopPropagation()} onClick={() => { setCatalogOpen((value) => !value); setManualCatalog(true); setCatalogLayer("root"); setCatalogQuery(""); }}><Plus size={17} /></button><textarea value={draft} onChange={(event) => { setDraft(event.target.value); event.currentTarget.style.height = "auto"; event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 168)}px`; }} onKeyDown={(event) => { if (event.key === "ArrowUp" && !event.shiftKey && !event.altKey && !event.metaKey) { event.preventDefault(); navigateComposerHistory("up"); return; } if (event.key === "ArrowDown" && !event.shiftKey && !event.altKey && !event.metaKey) { event.preventDefault(); navigateComposerHistory("down"); return; } if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); sendMessage(); } }} placeholder="Ask RIGA to make a change…" rows={1} /><button className={`send-button ${isRunning ? "stop-ready" : draft.trim() ? "send-ready" : ""}`} aria-label={isRunning ? "Stop run" : "Send message"} onClick={isRunning ? stopRun : sendMessage}>{isRunning ? <CircleStop size={16} /> : <Send size={16} />}</button></div>{catalogOpen && <div className="catalog-menu" ref={catalogRef} role="listbox">
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

function ToolCallView({ item }: { item: Extract<TranscriptItem, { role: "tool" }> }) {
  const terminal = item.name === "bash" || item.name === "shell";
  return <details className={`tool-card ${terminal ? "terminal-tool" : ""}`} open>
    <summary className="tool-card-top">
      <div className="tool-symbol"><TerminalSquare size={15} /></div>
      <div><strong>{item.name}</strong><span>{item.command}</span></div>
      <span className={`tool-status ${item.status}`}><span /> {item.status === "running" ? "Running" : item.status === "error" ? "Failed" : "Completed"}</span>
    </summary>
    <div className={`tool-output ${terminal ? "terminal-output" : ""}`}><span className="tool-output-label">{terminal ? "Terminal output" : item.status === "error" ? "Tool error" : "Tool result"}</span>{item.output}</div>
  </details>;
}

function TranscriptItemView({ item }: { item: TranscriptItem }) {
  if (item.role === "tool") return <ToolCallView item={item} />;
  // Only the assistant's prose is rendered as markdown. A user message is echoed
  // back verbatim on purpose: parsing it would reflow what was literally typed,
  // and the system lines are fixed UI strings with no markdown in them.
  const body = item.role === "assistant"
    ? <Markdown text={item.text} />
    : <p>{item.text}</p>;
  return <article className={`message-row ${item.role}`}><div className="message-avatar">{item.role === "assistant" ? <Bot size={15} /> : item.role === "system" ? <ShieldCheck size={15} /> : "AM"}</div><div className="message-body"><div className="message-meta"><strong>{item.role === "assistant" ? "RIGA" : item.role === "system" ? "System" : "You"}</strong><span>{item.time}</span></div>{body}</div></article>;
}

export default AssistantUI;
