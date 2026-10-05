import { useEffect, useMemo, useRef, useState, type SetStateAction } from "react";
import { createRoot } from "react-dom/client";
import { RIGA_ASSISTANT_UI_VERSION } from "@riga/assistant-ui";
import { RigaWebSocketClient, type RigaEventEnvelope } from "@riga/transport-http";
import {
  Bell,
  Bot,
  Check,
  ChevronDown,
  CircleStop,
  Clock3,
  Code2,
  FolderOpen,
  GitBranch,
  Menu,
  MessageSquare,
  MoreHorizontal,
  Paperclip,
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
  | { id: string; role: "tool"; name: string; command: string; status: "running" | "done"; output: string; time: string };

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

function App() {
  const [sessions, setSessions] = useState(initialSessions);
  const [sessionTranscripts, setSessionTranscripts] = useState(sessionHistories);
  const [draft, setDraft] = useState("");
  const [isRunning, setIsRunning] = useState(false);
  const [approval, setApproval] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [showSearch, setShowSearch] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [providerEndpoint, setProviderEndpoint] = useState("");
  const [providerApiKey, setProviderApiKey] = useState("");
  const [providerModel, setProviderModel] = useState("");
  const [providerMode, setProviderMode] = useState<"remote" | "local">("remote");
  const [transportStatus, setTransportStatus] = useState<"connecting" | "connected" | "closed" | "error">("connecting");
  const transportRef = useRef<RigaWebSocketClient | null>(null);

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
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const client = new RigaWebSocketClient({
      url: `${protocol}//${window.location.host}/ws`,
      onStatus: setTransportStatus,
      onError: (code, message) => setTranscript((current) => [...current, { id: crypto.randomUUID(), role: "system", text: `${code}: ${message}`, time: "now" }]),
      onEvent: (envelope: RigaEventEnvelope) => {
        const event = envelope.event;
        if (typeof event === "object" && event !== null && "TextDelta" in event) {
          const delta = (event as { TextDelta: { delta: string } }).TextDelta.delta;
          setTranscript((current) => [...current, { id: envelope.event_id, role: "assistant", text: delta, time: "now" }]);
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
    setTranscript((current) => [...current, { id: crypto.randomUUID(), role: "user", text, time: "now" }]);
    setDraft("");
    setIsRunning(true);
    setApproval(false);
    if (transportRef.current && transportStatus === "connected") {
      void transportRef.current.startRun(crypto.randomUUID(), activeSession.id, text).catch(() => {
        setTransportStatus("error");
        setIsRunning(false);
      });
      return;
    }
    setIsRunning(false);
    setTranscript((current) => [...current, { id: crypto.randomUUID(), role: "system", text: "WebSocket is not connected. Configure a provider and wait for the connection before sending.", time: "now" }]);
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

  async function saveProvider() {
    if (providerMode === "local") {
      setToast("Local GGUF downloads and inference are available in the Tauri desktop build; browser mode uses an OpenAI-compatible endpoint.");
      return;
    }
    if (!providerEndpoint.trim() || !providerModel.trim()) {
      setToast("Endpoint and model are required");
      return;
    }
    await transportRef.current?.configureProvider(providerEndpoint.trim(), providerApiKey, providerModel.trim());
    setSettingsOpen(false);
    setToast(`Provider configured in memory: ${providerModel.trim()}`);
  }

  return (
    <div className="app-shell">
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
          <div className="topbar-actions"><div className="run-indicator"><span className={isRunning ? "pulse-dot" : "online-dot"} /> {isRunning ? "Run in progress" : "Ready"}</div><button className="icon-button" aria-label="More options"><MoreHorizontal size={19} /></button><div className="avatar">AM</div></div>
        </header>

        {settingsOpen && <section className="settings-panel"><div className="settings-panel-header"><div><p className="eyebrow">RUNTIME / PROVIDER</p><h2>Connect your model.</h2><p>Credentials stay in browser memory and are sent only over the current WebSocket session.</p></div><button className="icon-button" aria-label="Close settings" onClick={() => setSettingsOpen(false)}><X size={17} /></button></div><div className="provider-tabs"><button className={providerMode === "remote" ? "selected" : ""} onClick={() => setProviderMode("remote")}>OpenAI-compatible / OpenCode Go</button><button className={providerMode === "local" ? "selected" : ""} onClick={() => setProviderMode("local")}>Local GGUF model</button></div>{providerMode === "remote" ? <div className="provider-form"><label>API endpoint<input value={providerEndpoint} onChange={(event) => setProviderEndpoint(event.target.value)} placeholder="https://api.example.com/v1" /></label><label>API key <span>ephemeral</span><input type="password" value={providerApiKey} onChange={(event) => setProviderApiKey(event.target.value)} placeholder="sk-…" autoComplete="off" /></label><label>Model<input value={providerModel} onChange={(event) => setProviderModel(event.target.value)} placeholder="opencode-go / gpt-4o-mini" /></label><button className="approve-button settings-save" onClick={() => void saveProvider()}><Check size={15} /> Save in memory</button></div> : <div className="local-model-card"><div className="tool-symbol"><Bot size={17} /></div><div><strong>Download and run a GGUF model locally</strong><p>Inspired by Fina Builder: model downloads, SHA-256 verification, CPU/OpenMP, and optional CUDA builds belong to the Tauri desktop runtime. This browser session cannot access the host filesystem or GPU.</p><button className="outline-button" onClick={() => setToast("Use the Tauri desktop build to download and run local GGUF models.")}>Open desktop model manager</button></div></div>}</section>}

        <section className="run-strip"><div className="run-strip-main"><div className="run-icon"><Sparkles size={16} /></div><div><strong>Agent run</strong><span>{isRunning ? "Executing with configured provider" : approval ? "Awaiting approval" : "Ready for your next instruction"}</span></div></div><div className="run-strip-meta"><span><GitBranch size={14} /> main</span><span><Clock3 size={14} /> 00:42</span>{isRunning && <button className="stop-run" onClick={() => { void transportRef.current?.cancelRun("active-run"); setIsRunning(false); setToast("Run cancelled safely"); }}><CircleStop size={14} /> Stop</button>}</div></section>

        <section className="content-column">
          <div className="conversation-header"><div><p className="eyebrow">SESSION / {activeSession.id.toUpperCase()}</p><h1>Build with confidence.</h1><p className="subtitle">A durable, inspectable coding-agent workspace.</p></div><button className="outline-button"><TerminalSquare size={15} /> Open terminal</button></div>
          <div className="transcript" aria-live="polite">
            {transcript.length === 0 && <div className="empty-state"><div className="empty-icon"><Bot size={26} /></div><h2>Start a coding run</h2><p>Describe the change, then review every tool action before it touches your workspace.</p></div>}
            {transcript.map((item) => <TranscriptItemView key={item.id} item={item} />)}
            {isRunning && <div className="typing-row"><div className="assistant-badge"><Bot size={15} /></div><div className="typing-bubble"><span /><span /><span /></div><small>RIGA is thinking</small></div>}
          </div>

          {approval && <div className="approval-card"><div className="approval-icon"><ShieldCheck size={19} /></div><div className="approval-copy"><div className="approval-title"><strong>Approval required</strong><span>workspace mutation</span></div><p>Allow RIGA to write the transport adapter boundary in <code>crates/</code> and update the event journal contract?</p><div className="approval-details"><span><FolderOpen size={13} /> 3 files</span><span><GitBranch size={13} /> reversible change</span><span><Clock3 size={13} /> requested now</span></div></div><div className="approval-actions"><button className="deny-button" onClick={deny}>Decline</button><button className="approve-button" onClick={approve}><Check size={15} /> Approve</button></div></div>}

          <div className="composer-wrap"><div className="composer"><button className="icon-button composer-icon" aria-label="Attach file"><Paperclip size={17} /></button><textarea value={draft} onChange={(event) => setDraft(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); sendMessage(); } }} placeholder="Ask RIGA to make a change…" rows={1} /><button className={`send-button ${draft.trim() ? "send-ready" : ""}`} aria-label="Send message" onClick={sendMessage}><Send size={16} /></button></div><div className="composer-footer"><span><kbd>Enter</kbd> send · <kbd>Shift Enter</kbd> newline</span><span>RIGA Kernel · local</span></div></div>
        </section>
      </main>
      {toast && <button className="toast" onClick={() => setToast(null)}><Check size={15} /> {toast}</button>}
    </div>
  );
}

function TranscriptItemView({ item }: { item: TranscriptItem }) {
  if (item.role === "tool") return <article className="tool-card"><div className="tool-card-top"><div className="tool-symbol"><TerminalSquare size={15} /></div><div><strong>{item.name}</strong><span>{item.command}</span></div><span className={`tool-status ${item.status}`}><span /> {item.status === "running" ? "Running" : "Completed"}</span></div><div className="tool-output">{item.output}</div></article>;
  return <article className={`message-row ${item.role}`}><div className="message-avatar">{item.role === "assistant" ? <Bot size={15} /> : item.role === "system" ? <ShieldCheck size={15} /> : "AM"}</div><div className="message-body"><div className="message-meta"><strong>{item.role === "assistant" ? "RIGA" : item.role === "system" ? "System" : "You"}</strong><span>{item.time}</span></div><p>{item.text}</p></div></article>;
}

export default App;

createRoot(document.getElementById("root")!).render(<App />);
