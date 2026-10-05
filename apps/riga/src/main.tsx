import { useMemo, useState } from "react";
import { RIGA_ASSISTANT_UI_VERSION } from "@riga/assistant-ui";
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

function App() {
  const [sessions, setSessions] = useState(initialSessions);
  const [transcript, setTranscript] = useState(initialTranscript);
  const [draft, setDraft] = useState("");
  const [isRunning, setIsRunning] = useState(false);
  const [approval, setApproval] = useState(true);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [showSearch, setShowSearch] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  const activeSession = useMemo(() => sessions.find((session) => session.active) ?? sessions[0], [sessions]);

  function selectSession(id: string) {
    setSessions((current) => current.map((session) => ({ ...session, active: session.id === id })));
    setSidebarOpen(false);
  }

  function createSession() {
    const next = { id: `session-${sessions.length + 1}`, title: "New coding session", meta: "Just now · 0 messages", active: true };
    setSessions((current) => [next, ...current.map((session) => ({ ...session, active: false }))]);
    setTranscript([]);
    setApproval(false);
    setToast("New session created");
  }

  function sendMessage() {
    const text = draft.trim();
    if (!text || isRunning) return;
    setTranscript((current) => [...current, { id: crypto.randomUUID(), role: "user", text, time: "now" }]);
    setDraft("");
    setIsRunning(true);
    window.setTimeout(() => {
      setTranscript((current) => [
        ...current,
        { id: crypto.randomUUID(), role: "assistant", text: "I’ve queued that request in the active run. I’ll keep the next filesystem action behind an approval checkpoint.", time: "now" },
      ]);
      setIsRunning(false);
      setApproval(true);
    }, 650);
  }

  function approve() {
    setApproval(false);
    setIsRunning(true);
    setTranscript((current) => [
      ...current,
      { id: crypto.randomUUID(), role: "system", text: "Approval granted · transport adapter implementation may proceed.", time: "now" },
      { id: crypto.randomUUID(), role: "tool", name: "workspace.write", command: "riga workspace apply --approved", status: "running", output: "Writing adapter boundary…", time: "now" },
    ]);
    window.setTimeout(() => {
      setTranscript((current) => current.map((item) => item.role === "tool" && item.status === "running" ? { ...item, status: "done", output: "Adapter boundary written · checks pending" } : item));
      setIsRunning(false);
      setToast("Approval applied");
    }, 900);
  }

  function deny() {
    setApproval(false);
    setToast("Approval declined; no workspace mutation was made");
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
          <div className="connection-status"><span className="online-dot" /> Local kernel connected · UI {RIGA_ASSISTANT_UI_VERSION}</div>
          <button className="footer-link"><Settings2 size={15} /> Settings <span>⌘ ,</span></button>
          <button className="footer-link"><Bell size={15} /> Notifications <span className="notification-count">2</span></button>
        </div>
      </aside>

      {sidebarOpen && <button className="mobile-scrim" aria-label="Close navigation" onClick={() => setSidebarOpen(false)} />}

      <main className="main-panel">
        <header className="topbar">
          <div className="topbar-left"><button className="icon-button menu-button" aria-label="Open navigation" onClick={() => setSidebarOpen(true)}><Menu size={19} /></button><div className="breadcrumbs"><span>Workspace</span><span>/</span><strong>{activeSession.title}</strong></div></div>
          <div className="topbar-actions"><div className="run-indicator"><span className={isRunning ? "pulse-dot" : "online-dot"} /> {isRunning ? "Run in progress" : "Ready"}</div><button className="icon-button" aria-label="More options"><MoreHorizontal size={19} /></button><div className="avatar">AM</div></div>
        </header>

        <section className="run-strip"><div className="run-strip-main"><div className="run-icon"><Sparkles size={16} /></div><div><strong>Agent run</strong><span>{isRunning ? "Executing with approval policy" : "Paused at approval checkpoint"}</span></div></div><div className="run-strip-meta"><span><GitBranch size={14} /> main</span><span><Clock3 size={14} /> 00:42</span>{isRunning && <button className="stop-run" onClick={() => { setIsRunning(false); setToast("Run cancelled safely"); }}><CircleStop size={14} /> Stop</button>}</div></section>

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
