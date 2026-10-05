import { useEffect, useMemo, useRef, useState, type SetStateAction } from "react";
import { createRoot } from "react-dom/client";
import { RIGA_ASSISTANT_UI_VERSION } from "@haymant/assistant-ui";
import { RigaWebSocketClient, type RigaEventEnvelope } from "@haymant/transport-http";
import {
  Bell,
  Bot,
  Check,
  ChevronDown,
  ChevronLeft,
  CircleStop,
  Clock3,
  Code2,
  FolderOpen,
  GitBranch,
  Menu,
  MessageSquare,
  MoreHorizontal,
  Paperclip,
  Pencil,
  Moon,
  Sun,
  Play,
  Plus,
  Search,
  Send,
  Settings2,
  ShieldCheck,
  Sparkles,
  TerminalSquare,
  X,
} from "lucide-react";
import "./styles.css";

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
type McpServerSummary = { name: string; command: string; tools: string[]; transport?: "stdio" | "http"; url?: string };
type ConnectorDraft = { name: string; transport: "stdio" | "http"; command: string; args: string; url: string };
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

function App() {
  const [sessions, setSessions] = useState<Session[]>(() => loadLocal("riga.sessions.v1", initialSessions));
  const [sessionTranscripts, setSessionTranscripts] = useState<Record<string, TranscriptItem[]>>(() => loadLocal("riga.transcripts.v1", sessionHistories));
  const [draft, setDraft] = useState("");
  const [composerHistory, setComposerHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [approval, setApproval] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [showSearch, setShowSearch] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [providerEndpoint, setProviderEndpoint] = useState("");
  const [providerApiKey, setProviderApiKey] = useState("");
  const [providerModel, setProviderModel] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffort>("low");
  const [providerMode, setProviderMode] = useState<"remote" | "local">("remote");
  const [theme, setTheme] = useState<"dark" | "light">(() => loadLocal("riga.theme.v1", "dark"));
  const [transportStatus, setTransportStatus] = useState<"connecting" | "connected" | "closed" | "error">("connecting");
  const [catalogOpen, setCatalogOpen] = useState(false);
  const [catalogLayer, setCatalogLayer] = useState<CatalogLayer>("root");
  const [catalogQuery, setCatalogQuery] = useState("");
  const [editingConnector, setEditingConnector] = useState<string | null>(null);
  const [connectorDraft, setConnectorDraft] = useState<ConnectorDraft>({ name: "", transport: "stdio", command: "", args: "", url: "" });
  const [catalogItems, setCatalogItems] = useState<CatalogItem[]>([]);
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [workspaceFiles, setWorkspaceFiles] = useState<FileCandidate[]>([]);
  const [mcpServers, setMcpServers] = useState<McpServerSummary[]>([]);
  const transportRef = useRef<RigaWebSocketClient | null>(null);
  const catalogRef = useRef<HTMLDivElement | null>(null);

  const activeSession = useMemo(() => sessions.find((session) => session.active) ?? sessions[0], [sessions]);
  const transcript = sessionTranscripts[activeSession.id] ?? [];
  const setTranscript = (updater: SetStateAction<TranscriptItem[]>) => {
    setSessionTranscripts((current) => {
      const previous = current[activeSession.id] ?? [];
      const next = typeof updater === "function" ? updater(previous) : updater;
      return { ...current, [activeSession.id]: next };
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

  const savedConnectors = loadLocal<McpServerSummary[]>("riga.connectors.v1", []);
  const allConnectors = useMemo(() => {
    const merged = [...savedConnectors, ...mcpServers];
    return merged.filter((server, index, list) => list.findIndex((candidate) => candidate.name === server.name) === index);
  }, [mcpServers, savedConnectors]);

  const triggerMatch = useMemo(() => {
    const match = draft.match(/(?:^|\s)([@/])([^\s]*)$/);
    return match ? { char: match[1], query: match[2] } : null;
  }, [draft]);

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
      onProviderConfigured: (endpoint, model, effort) => {
        setProviderEndpoint(endpoint);
        setProviderModel(model);
        setReasoningEffort(effort);
      },
      onError: (code, message) => setTranscript((current) => [...current, { id: crypto.randomUUID(), role: "system", text: `${code}: ${message}`, time: "now" }]),
      onEvent: (envelope: RigaEventEnvelope) => {
        const event = envelope.event;
        if (typeof event === "object" && event !== null && "ToolCallStarted" in event) {
          const call = (event as { ToolCallStarted: { call: { call_id?: string; name?: string; arguments?: unknown } } }).ToolCallStarted.call;
          setTranscript((current) => [...current, { id: envelope.event_id, role: "tool", callId: call.call_id, name: call.name ?? "tool", command: typeof call.arguments === "string" ? call.arguments : JSON.stringify(call.arguments ?? {}), status: "running", output: "Waiting for result…", time: "now" }]);
        } else if (typeof event === "object" && event !== null && "ToolResult" in event) {
          const result = (event as { ToolResult: { result: { call_id?: string; name?: string; output?: string; ok?: boolean } } }).ToolResult.result;
          setTranscript((current) => current.map((item) => item.role === "tool" && item.callId === result.call_id ? { ...item, status: result.ok ? "done" : "error", output: result.output ?? "" } : item));
        } else if (typeof event === "object" && event !== null && "TextDelta" in event) {
          const delta = (event as { TextDelta: { delta: string } }).TextDelta.delta;
          setTranscript((current) => {
            const last = current.at(-1);
            if (last?.role === "assistant" && last.id.startsWith("stream-")) {
              return [...current.slice(0, -1), { ...last, text: `${last.text}${delta}` }];
            }
            return [...current, { id: `stream-${envelope.run_id}`, role: "assistant", text: delta, time: "now" }];
          });
        } else if (typeof event === "object" && event !== null && "RunCompleted" in event) {
          setIsRunning(false);
        } else if (typeof event === "object" && event !== null && "RunFailed" in event) {
          setIsRunning(false);
          setTranscript((current) => [...current, { id: envelope.event_id, role: "system", text: `Agent run failed: ${(event as { RunFailed: { message: string } }).RunFailed.message}`, time: "now" }]);
        } else if (event === "RunStarted") {
          setIsRunning(true);
        }
      },
    });
    transportRef.current = client;
    client.connect().catch(() => setTransportStatus("error"));
    return () => client.close();
  }, []);

  function selectSession(id: string) {
    setSessions((current) => current.map((session) => ({ ...session, active: session.id === id })));
    setSidebarOpen(false);
  }

  function createSession() {
    const next = { id: `session-${sessions.length + 1}`, title: "New coding session", meta: "Just now · 0 messages", active: true };
    setSessions((current) => [next, ...current.map((session) => ({ ...session, active: false }))]);
    setSessionTranscripts((current) => ({ ...current, [next.id]: [] }));
    setApproval(false);
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
      void transportRef.current.startRun(crypto.randomUUID(), activeSession.id, prompt).catch(() => {
        setTransportStatus("error");
        setIsRunning(false);
      });
      return;
    }
    setIsRunning(false);
    setTranscript((current) => [...current, { id: crypto.randomUUID(), role: "system", text: "WebSocket is not connected. Configure a provider and wait for the connection before sending.", time: "now" }]);
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
    setProviderModel(model);
    if (model && providerEndpoint.trim() && transportStatus === "connected") {
      void transportRef.current?.configureProvider(providerEndpoint.trim(), "", model, reasoningEffort);
    }
  }

  function insertCatalog(text: string) {
    setDraft((current) => {
      if (triggerMatch) return current.replace(/(?:^|\s)([@/])([^\s]*)$/, (match) => `${match.startsWith(" ") ? " " : ""}${text}`);
      return `${current}${current ? "\n" : ""}${text}`;
    });
    setCatalogOpen(false);
    setCatalogQuery("");
    setCatalogLayer("root");
  }

  function openConnectorEditor(server?: McpServerSummary) {
    setEditingConnector(server?.name ?? null);
    setConnectorDraft({ name: server?.name ?? "", transport: server?.transport ?? (server?.url ? "http" : "stdio"), command: server?.command === "unknown" ? "" : server?.command ?? "", args: "", url: server?.url ?? "" });
  }

  function saveConnector() {
    const name = connectorDraft.name.trim();
    if (!name || (connectorDraft.transport === "stdio" ? !connectorDraft.command.trim() : !connectorDraft.url.trim())) {
      setToast("Connector name and transport details are required");
      return;
    }
    const next: McpServerSummary = { name, command: connectorDraft.transport === "stdio" ? connectorDraft.command.trim() : "http-stream", tools: [], transport: connectorDraft.transport, url: connectorDraft.transport === "http" ? connectorDraft.url.trim() : undefined };
    const updated = [...allConnectors.filter((server) => server.name !== name), next];
    window.localStorage.setItem("riga.connectors.v1", JSON.stringify(updated));
    setMcpServers(updated);
    setEditingConnector(null);
    setToast(`Connector ${name} saved`);
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

  const menuTools = triggerMatch?.char === "@" ? [] : searchableItems.tools;
  const menuSkills = triggerMatch?.char === "@" ? [] : searchableItems.skills;
  const menuConnectors = triggerMatch?.char === "/" || triggerMatch?.char === "@" ? [] : searchableItems.connectors;

  async function saveProvider() {
    if (providerMode === "local") {
      setToast("Local GGUF downloads and inference are available in the Tauri desktop build; browser mode uses an OpenAI-compatible endpoint.");
      return;
    }
    if (!providerEndpoint.trim() || !providerModel.trim()) {
      setToast("Endpoint and model are required");
      return;
    }
    await transportRef.current?.configureProvider(providerEndpoint.trim(), providerApiKey, providerModel.trim(), reasoningEffort);
    setSettingsOpen(false);
    setToast(`Provider saved securely: ${providerModel.trim()}`);
  }

  return (
    <div className={`app-shell theme-${theme}`}>
      <aside className={`sidebar ${sidebarOpen ? "sidebar-open" : ""}`}>
        <div className="brand-row">
          <div className="brand-mark"><Code2 size={18} strokeWidth={2.6} /></div>
          <div><strong>RIGA</strong><span>coding agent</span></div>
          <button className="icon-button mobile-close" aria-label="Close navigation" onClick={() => setSidebarOpen(false)}><X size={17} /></button>
        </div>
        <button className="new-session" onClick={createSession}><Plus size={17} /> New session <span>⌘ K</span></button>
        <div className="sidebar-section-label">Workspace</div>
        <div className="workspace-card">
          <div className="workspace-icon"><FolderOpen size={16} /></div>
          <div><strong>riga</strong><span>~/projects/riga</span></div>
          <ChevronDown size={15} className="muted-icon" />
        </div>
        <div className="sidebar-section-label sessions-label">Sessions <button className="icon-button" aria-label="Search sessions" onClick={() => setShowSearch((value) => !value)}><Search size={15} /></button></div>
        {showSearch && <input className="session-search" placeholder="Filter sessions" autoFocus />}
        <nav className="session-list" aria-label="Sessions">
          {sessions.map((session) => (
            <button key={session.id} className={`session-item ${session.active ? "active" : ""}`} onClick={() => selectSession(session.id)}>
              <MessageSquare size={15} /><span><strong>{session.title}</strong><small>{session.meta}</small></span>{session.active && <span className="active-dot" />}
            </button>
          ))}
        </nav>
        <div className="sidebar-footer">
          <div className="connection-status"><span className={transportStatus === "connected" ? "online-dot" : "pulse-dot"} /> WebSocket {transportStatus} · UI {RIGA_ASSISTANT_UI_VERSION}</div>
          <button className="footer-link" onClick={() => setSettingsOpen((value) => !value)}><Settings2 size={15} /> Settings <span>⌘ ,</span></button>
          <button className="footer-link"><Bell size={15} /> Notifications <span className="notification-count">2</span></button>
        </div>
      </aside>

      {sidebarOpen && <button className="mobile-scrim" aria-label="Close navigation" onClick={() => setSidebarOpen(false)} />}

      <main className="main-panel">
        <header className="topbar">
          <div className="topbar-left"><button className="icon-button menu-button" aria-label="Open navigation" onClick={() => setSidebarOpen(true)}><Menu size={19} /></button><div className="breadcrumbs"><span>Workspace</span><span>/</span><strong>{activeSession.title}</strong></div></div>
          <div className="topbar-actions"><div className="run-indicator"><span className={isRunning ? "pulse-dot" : "online-dot"} /> {isRunning ? "Run in progress" : "Ready"}</div><button className="icon-button" aria-label="Toggle theme" onClick={() => setTheme((value) => value === "dark" ? "light" : "dark")}>{theme === "dark" ? <Sun size={17} /> : <Moon size={17} />}</button><button className="icon-button" aria-label="More options"><MoreHorizontal size={19} /></button><div className="avatar">AM</div></div>
        </header>

        {settingsOpen && <section className="settings-panel"><div className="settings-panel-header"><div><p className="eyebrow">RUNTIME / PROVIDER</p><h2>Connect your model.</h2><p>Endpoint and model restore after reload. The API key is sent over WebSocket and retained only in the server's encrypted store.</p></div><button className="icon-button" aria-label="Close settings" onClick={() => setSettingsOpen(false)}><X size={17} /></button></div><div className="provider-tabs"><button className={providerMode === "remote" ? "selected" : ""} onClick={() => setProviderMode("remote")}>OpenAI-compatible / OpenCode Go</button><button className={providerMode === "local" ? "selected" : ""} onClick={() => setProviderMode("local")}>Local GGUF model</button></div>{providerMode === "remote" ? <div className="provider-form"><label>API endpoint<input value={providerEndpoint} onChange={(event) => setProviderEndpoint(event.target.value)} placeholder="https://api.example.com/v1" /></label><label>API key <span>encrypted at rest</span><input type="password" value={providerApiKey} onChange={(event) => setProviderApiKey(event.target.value)} placeholder="sk-…" autoComplete="off" /></label><label>Model<input value={providerModel} onChange={(event) => setProviderModel(event.target.value)} placeholder="opencode-go / gpt-4o-mini" /></label><label>Reasoning effort<select value={reasoningEffort} onChange={(event) => setReasoningEffort(event.target.value as ReasoningEffort)}><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option></select></label><button className="approve-button settings-save" onClick={() => void saveProvider()}><Check size={15} /> Save securely</button></div> : <div className="local-model-card"><div className="tool-symbol"><Bot size={17} /></div><div><strong>Download and run a GGUF model locally</strong><p>Inspired by Fina Builder: model downloads, SHA-256 verification, CPU/OpenMP, and optional CUDA builds belong to the Tauri desktop runtime. This browser session cannot access the host filesystem or GPU.</p><button className="outline-button" onClick={() => setToast("Use the Tauri desktop build to download and run local GGUF models.")}>Open desktop model manager</button></div></div>}</section>}

        <section className="run-strip"><div className="run-strip-main"><div className="run-icon"><Sparkles size={16} /></div><div><strong>Agent run</strong><span>{isRunning ? "Executing with configured provider" : approval ? "Awaiting approval" : "Ready for your next instruction"}</span></div></div><div className="run-strip-meta"><span><GitBranch size={14} /> main</span><span><Clock3 size={14} /> 00:42</span>{isRunning && <button className="stop-run" onClick={() => { void transportRef.current?.cancelRun("active-run"); setIsRunning(false); setToast("Run cancelled safely"); }}><CircleStop size={14} /> Stop</button>}</div></section>

        <section className="content-column">
          <div className="conversation-header"><div><p className="eyebrow">SESSION / {activeSession.id.toUpperCase()}</p><h1>Build with confidence.</h1><p className="subtitle">A durable, inspectable coding-agent workspace.</p></div><button className="outline-button"><TerminalSquare size={15} /> Open terminal</button></div>
          <div className="transcript" aria-live="polite">
            {transcript.length === 0 && <div className="empty-state"><div className="empty-icon"><Bot size={26} /></div><h2>Start a coding run</h2><p>Describe the change, then review every tool action before it touches your workspace.</p></div>}
            {groupTranscript(transcript).map((block) => block.role === "timeline" ? <ToolTimeline key={block.id} items={block.items} /> : <TranscriptItemView key={block.id} item={block} />)}
            {isRunning && <div className="typing-row"><div className="assistant-badge"><Bot size={15} /></div><div className="typing-bubble"><span /><span /><span /></div><small>RIGA is thinking</small></div>}
          </div>

          {approval && <div className="approval-card"><div className="approval-icon"><ShieldCheck size={19} /></div><div className="approval-copy"><div className="approval-title"><strong>Approval required</strong><span>workspace mutation</span></div><p>Allow RIGA to write the transport adapter boundary in <code>crates/</code> and update the event journal contract?</p><div className="approval-details"><span><FolderOpen size={13} /> 3 files</span><span><GitBranch size={13} /> reversible change</span><span><Clock3 size={13} /> requested now</span></div></div><div className="approval-actions"><button className="deny-button" onClick={deny}>Decline</button><button className="approve-button" onClick={approve}><Check size={15} /> Approve</button></div></div>}

          <div className="composer-wrap">{attachments.length > 0 && <div className="composer-attachments">{attachments.map((attachment) => <span className="attachment-chip" key={attachment.path}><Paperclip size={12} /> {attachment.name}<button type="button" aria-label={`Remove ${attachment.name}`} onClick={() => setAttachments((current) => current.filter((item) => item.path !== attachment.path))}><X size={12} /></button></span>)}</div>}<div className="composer"><input ref={fileInputRef} className="file-input-hidden" type="file" multiple onChange={(event) => { void uploadAttachments(event.target.files); event.currentTarget.value = ""; }} /><button className="icon-button composer-icon" aria-label="Attach file" onClick={() => fileInputRef.current?.click()}><Paperclip size={17} /></button><div className="composer-model"><select aria-label="Configured model" value={providerModel} onChange={(event) => selectComposerModel(event.target.value)}><option value="">Model</option>{Array.from(new Set([providerModel, "gpt-5-nano", "gpt-5-mini", "gpt-5-codex"])).filter(Boolean).map((model) => <option key={model} value={model}>{model}</option>)}</select><select aria-label="Reasoning effort" value={reasoningEffort} onChange={(event) => setReasoningEffort(event.target.value as ReasoningEffort)}><option value="low">Low</option><option value="medium">Medium</option><option value="high">High</option></select></div><button className="icon-button composer-plus" aria-label="Insert tool, skill, or MCP" onPointerDown={(event) => event.stopPropagation()} onClick={() => { setCatalogOpen((value) => !value); setCatalogLayer("root"); setCatalogQuery(""); }}><Plus size={17} /></button><textarea value={draft} onChange={(event) => { setDraft(event.target.value); event.currentTarget.style.height = "auto"; event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 168)}px`; }} onKeyDown={(event) => { if (event.key === "ArrowUp" && !event.shiftKey && !event.altKey && !event.metaKey) { event.preventDefault(); navigateComposerHistory("up"); return; } if (event.key === "ArrowDown" && !event.shiftKey && !event.altKey && !event.metaKey) { event.preventDefault(); navigateComposerHistory("down"); return; } if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); sendMessage(); } }} placeholder="Ask RIGA to make a change…" rows={1} /><button className={`send-button ${draft.trim() ? "send-ready" : ""}`} aria-label="Send message" onClick={sendMessage}><Send size={16} /></button></div>{catalogOpen && <div className="catalog-menu" ref={catalogRef} role="listbox">
              <div className="catalog-menu-header">{catalogLayer === "connectors" && <button className="catalog-back" aria-label="Back to insert menu" onClick={() => setCatalogLayer("root")}><ChevronLeft size={14} /></button>}<strong>{triggerMatch ? `${triggerMatch.char === "@" ? "Mention" : "Command"} suggestions` : catalogLayer === "root" ? "Insert into composer" : "Connectors"}</strong><button className="catalog-close" aria-label="Close insert menu" onClick={() => setCatalogOpen(false)}><X size={14} /></button></div>
              <input className="catalog-search" autoFocus={catalogOpen} value={catalogQuery} onChange={(event) => setCatalogQuery(event.target.value)} placeholder={triggerMatch ? `Filter ${triggerMatch.char === "@" ? "files or agents" : "tools and skills"}…` : "Search tools, skills, connectors…"} aria-label="Search composer insert menu" />
              {editingConnector ? <div className="connector-form"><label>Name<input value={connectorDraft.name} onChange={(event) => setConnectorDraft({ ...connectorDraft, name: event.target.value })} placeholder="my-server" /></label><label>Transport<select value={connectorDraft.transport} onChange={(event) => setConnectorDraft({ ...connectorDraft, transport: event.target.value as "stdio" | "http" })}><option value="stdio">stdio process</option><option value="http">HTTP stream</option></select></label>{connectorDraft.transport === "stdio" ? <><label>Command<input value={connectorDraft.command} onChange={(event) => setConnectorDraft({ ...connectorDraft, command: event.target.value })} placeholder="npx -y @modelcontextprotocol/server-filesystem" /></label><label>Arguments<input value={connectorDraft.args} onChange={(event) => setConnectorDraft({ ...connectorDraft, args: event.target.value })} placeholder="/workspace" /></label></> : <label>HTTP stream URL<input value={connectorDraft.url} onChange={(event) => setConnectorDraft({ ...connectorDraft, url: event.target.value })} placeholder="https://example.com/mcp" /></label>}<div className="connector-form-actions"><button className="outline-button" onClick={() => setEditingConnector(null)}>Cancel</button><button className="approve-button" onClick={saveConnector}><Check size={14} /> Save connector</button></div></div> : <>{catalogLayer === "root" && !triggerMatch && !catalogQuery && <><small>Connectors</small><button className="catalog-category" onClick={() => setCatalogLayer("connectors")}><span><FolderOpen size={14} /> MCP connectors</span><em>{allConnectors.length} registered <ChevronDown size={13} /></em></button><small>Built-in tools</small></>}{(catalogLayer === "connectors" || catalogQuery || triggerMatch?.char === "/" || triggerMatch?.char === "@") && <>{catalogLayer === "connectors" && <div className="catalog-inline-actions"><button className="catalog-category" onClick={() => openConnectorEditor()}><span><Plus size={14} /> Add connector</span><em>stdio or HTTP stream</em></button></div>}{menuConnectors.map((server) => <button className="catalog-item" key={`connector-${server.name}`} onClick={() => insertCatalog(`Use MCP server ${server.name}: `)}><span>mcp/{server.name}</span><em>{server.transport === "http" ? server.url : server.command}<button type="button" className="catalog-edit" aria-label={`Edit ${server.name}`} onClick={(event) => { event.stopPropagation(); openConnectorEditor(server); }}><Pencil size={12} /></button></em></button>)}</>}{(catalogLayer === "root" || catalogLayer === "connectors" || catalogQuery || triggerMatch) && <>{menuTools.length > 0 && <small>Built-in tools</small>}{menuTools.map((item) => <button className="catalog-item" key={item.id} onClick={() => insertCatalog(triggerMatch?.char === "@" ? `@${item.id} ` : triggerMatch?.char === "/" ? `/${item.id} ` : item.insert_text)}><span>{triggerMatch?.char === "/" ? `/${item.id}` : item.id}</span><em>{item.description}</em></button>)}{menuSkills.length > 0 && <small>Skills and agents</small>}{menuSkills.map((skill) => <button className="catalog-item" key={skill.name} onClick={() => insertCatalog(triggerMatch?.char === "@" ? `@${skill.name} ` : triggerMatch?.char === "/" ? `/${skill.name} ` : `Use the skill tool with name ${skill.name}: `)}><span>{triggerMatch?.char === "@" ? `@${skill.name}` : triggerMatch?.char === "/" ? `/${skill.name}` : `skill/${skill.name}`}</span><em>{skill.description}</em></button>)}{triggerMatch?.char === "@" && searchableItems.agents.length > 0 && <><small>Agents</small>{searchableItems.agents.map((agent) => <button className="catalog-item" key={`agent-${agent.name}`} onClick={() => insertCatalog(`@${agent.name} `)}><span>@{agent.name}</span><em>{agent.purpose}</em></button>)}</>}{triggerMatch?.char === "@" && searchableItems.files.length > 0 && <><small>Files</small>{searchableItems.files.map((file) => <button className="catalog-item" key={file.path} onClick={() => insertCatalog(`@${file.path} `)}><span>@{file.name}</span><em>{file.path}</em></button>)}</>}</>}</>}
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
  return <article className={`message-row ${item.role}`}><div className="message-avatar">{item.role === "assistant" ? <Bot size={15} /> : item.role === "system" ? <ShieldCheck size={15} /> : "AM"}</div><div className="message-body"><div className="message-meta"><strong>{item.role === "assistant" ? "RIGA" : item.role === "system" ? "System" : "You"}</strong><span>{item.time}</span></div><p>{item.text}</p></div></article>;
}

export default App;

createRoot(document.getElementById("root")!).render(<App />);
