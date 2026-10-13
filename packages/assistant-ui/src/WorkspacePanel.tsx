import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  Check,
  FolderTree,
  GitBranch,
  GitCommitHorizontal,
  Loader2,
  RefreshCw,
  Upload,
  X,
} from "lucide-react";
import { DiffViewer } from "./DiffViewer";
import { FileTree } from "./FileTree";
import { Markdown } from "./Markdown";
import { useResizablePopover } from "./useResizablePopover";
import type {
  GitChange,
  GitCommit,
  RigaTransport,
  WorkspaceDiff,
  WorkspaceEntry,
  WorkspaceFile,
} from "./protocol";

export type WorkspacePanelProps = {
  view: "files" | "git";
  sessionId: string;
  transport: RigaTransport | null;
  onClose: () => void;
  onToast: (message: string) => void;
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} kB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatWhen(timestamp: number): string {
  if (!timestamp) return "";
  const date = new Date(timestamp * 1000);
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

const LANGUAGES: Record<string, string> = {
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "jsx",
  ts: "typescript",
  tsx: "tsx",
  py: "python",
  rs: "rust",
  md: "markdown",
  mdx: "markdown",
  json: "json",
  yml: "yaml",
  yaml: "yaml",
  toml: "toml",
  css: "css",
  scss: "scss",
  html: "html",
  sh: "bash",
  go: "go",
  java: "java",
  c: "c",
  cpp: "cpp",
  sql: "sql",
};

function languageOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";
  return LANGUAGES[ext] ?? (ext || "text");
}

function prettyJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

function FilePreview({ file }: { file: WorkspaceFile }) {
  if (file.binary) {
    return (
      <div className="workspace-preview-empty">
        Binary file · {formatBytes(file.size)} — no text preview.
      </div>
    );
  }
  const language = languageOf(file.path);
  if (language === "markdown") {
    return (
      <div className="workspace-preview-scroll markdown-preview">
        <Markdown text={file.text} />
      </div>
    );
  }
  const text = language === "json" ? prettyJson(file.text) : file.text;
  const lines = text.split("\n");
  return (
    <div className="workspace-preview-scroll workspace-code">
      <div className="workspace-code-bar">
        <span>{language}</span>
        <span>
          {formatBytes(file.size)}
          {file.truncated ? " · truncated" : ""}
        </span>
      </div>
      <div className="workspace-code-body">
        {lines.map((line, index) => (
          // Line numbers are content, so an index key is correct here.
          <div className="workspace-code-line" key={index}>
            <span className="workspace-code-gutter">{index + 1}</span>
            <span className="workspace-code-text">{line || " "}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

const STATUS_LABEL: Record<string, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
  untracked: "U",
  conflicted: "!",
};

export function WorkspacePanel({ view, sessionId, transport, onClose, onToast }: WorkspacePanelProps) {
  const popover = useResizablePopover(view === "files" ? "files" : "git", { width: 1040, height: 640 });
  const splitRef = useRef<HTMLDivElement | null>(null);
  const gitColumnRef = useRef<HTMLDivElement | null>(null);
  const [splitRatio, setSplitRatio] = useState(1 / 3);
  const [gitTopHeight, setGitTopHeight] = useState(300);

  // Files view state.
  const [entries, setEntries] = useState<Record<string, WorkspaceEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [loadingDirs, setLoadingDirs] = useState<Set<string>>(new Set());
  const [selectedPath, setSelectedPath] = useState<string | undefined>();
  const [file, setFile] = useState<WorkspaceFile | null>(null);

  // Git view state.
  const [changes, setChanges] = useState<GitChange[]>([]);
  const [commits, setCommits] = useState<GitCommit[]>([]);
  const [selectedCommit, setSelectedCommit] = useState<string | undefined>();
  const [diff, setDiff] = useState<WorkspaceDiff | null>(null);
  const [commitMessage, setCommitMessage] = useState("");
  const [gitBusy, setGitBusy] = useState(false);

  const [error, setError] = useState<string | null>(null);

  const loadDirectory = useCallback(
    async (path: string) => {
      if (!transport || !sessionId) return;
      setLoadingDirs((current) => new Set(current).add(path));
      try {
        const children = await transport.workspaceEntries(sessionId, path);
        setEntries((current) => ({ ...current, [path]: children }));
        setError(null);
      } catch (loadError) {
        const message = loadError instanceof Error ? loadError.message : `Could not list ${path || "."}`;
        setError(message);
        onToast(message);
      } finally {
        setLoadingDirs((current) => {
          const next = new Set(current);
          next.delete(path);
          return next;
        });
      }
    },
    [transport, sessionId, onToast],
  );

  const loadStatus = useCallback(async () => {
    if (!transport || !sessionId) return;
    try {
      setChanges(await transport.gitStatus(sessionId));
      setError(null);
    } catch (loadError) {
      const message = loadError instanceof Error ? loadError.message : "Could not read git status";
      setError(message);
      onToast(message);
    }
  }, [transport, sessionId, onToast]);

  const loadLog = useCallback(async () => {
    if (!transport || !sessionId) return;
    try {
      setCommits(await transport.gitLog(sessionId, 60));
    } catch (loadError) {
      const message = loadError instanceof Error ? loadError.message : "Could not read git history";
      setError(message);
      onToast(message);
    }
  }, [transport, sessionId, onToast]);

  const refreshGit = useCallback(() => {
    void loadStatus();
    void loadLog();
  }, [loadStatus, loadLog]);

  // Reset when the pane or session changes, then load its first level.
  useEffect(() => {
    setEntries({});
    setExpanded(new Set());
    setSelectedPath(undefined);
    setSelectedCommit(undefined);
    setFile(null);
    setDiff(null);
    setError(null);
    setChanges([]);
    setCommits([]);
    if (!transport || !sessionId) return;
    if (view === "files") void loadDirectory("");
    else refreshGit();
  }, [view, sessionId, transport, loadDirectory, refreshGit]);

  const toggleDir = useCallback(
    (path: string) => {
      const isOpen = expanded.has(path);
      setExpanded((current) => {
        const next = new Set(current);
        if (isOpen) next.delete(path);
        else next.add(path);
        return next;
      });
      if (!isOpen && entries[path] === undefined) void loadDirectory(path);
    },
    [expanded, entries, loadDirectory],
  );

  const selectFile = useCallback(
    async (path: string) => {
      if (!transport || !sessionId) return;
      setSelectedPath(path);
      setDiff(null);
      setError(null);
      try {
        setFile(await transport.workspaceFile(sessionId, path));
      } catch (loadError) {
        setFile(null);
        setError(loadError instanceof Error ? loadError.message : "Could not read the file");
      }
    },
    [transport, sessionId],
  );

  const selectChange = useCallback(
    async (path: string) => {
      if (!transport || !sessionId) return;
      setSelectedPath(path);
      setSelectedCommit(undefined);
      setFile(null);
      setError(null);
      try {
        setDiff(await transport.gitDiff(sessionId, path));
      } catch (loadError) {
        setDiff(null);
        setError(loadError instanceof Error ? loadError.message : "Could not read the diff");
      }
    },
    [transport, sessionId],
  );

  const selectCommit = useCallback(
    async (commit: string) => {
      if (!transport || !sessionId) return;
      setSelectedCommit(commit);
      setSelectedPath(undefined);
      setFile(null);
      setError(null);
      try {
        setDiff(await transport.gitCommitDiff(sessionId, commit));
      } catch (loadError) {
        setDiff(null);
        setError(loadError instanceof Error ? loadError.message : "Could not read the commit");
      }
    },
    [transport, sessionId],
  );

  const commit = useCallback(async () => {
    if (!transport || !sessionId) return;
    if (!commitMessage.trim()) {
      onToast("Write a commit message first.");
      return;
    }
    setGitBusy(true);
    try {
      const result = await transport.gitCommit(sessionId, commitMessage.trim());
      onToast(result.message);
      if (result.committed) setCommitMessage("");
      refreshGit();
    } catch (commitError) {
      onToast(commitError instanceof Error ? commitError.message : "Commit failed");
    } finally {
      setGitBusy(false);
    }
  }, [transport, sessionId, commitMessage, onToast, refreshGit]);

  const push = useCallback(async () => {
    if (!transport || !sessionId) return;
    setGitBusy(true);
    try {
      const result = await transport.gitPush(sessionId);
      onToast(result.message);
      refreshGit();
    } catch (pushError) {
      onToast(pushError instanceof Error ? pushError.message : "Push failed");
    } finally {
      setGitBusy(false);
    }
  }, [transport, sessionId, onToast, refreshGit]);

  // Drag the vertical divider to change the list/diff split.
  const onSplitPointerDown = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const container = splitRef.current;
    if (!container) return;
    const bounds = container.getBoundingClientRect();
    const onMove = (move: PointerEvent) => {
      const ratio = (move.clientX - bounds.left) / bounds.width;
      setSplitRatio(Math.min(Math.max(ratio, 0.18), 0.7));
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }, []);

  // Drag the horizontal divider to change the changes/history split.
  const onGitSplitPointerDown = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const column = gitColumnRef.current;
    if (!column) return;
    const bounds = column.getBoundingClientRect();
    const onMove = (move: PointerEvent) => {
      const height = move.clientY - bounds.top;
      setGitTopHeight(Math.min(Math.max(height, 120), bounds.height - 120));
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }, []);

  const staged = useMemo(() => changes.filter((change) => change.staged), [changes]);
  const unstaged = useMemo(() => changes.filter((change) => !change.staged), [changes]);

  const title = view === "files" ? "Explorer" : "Source control";
  const emptyHint = !sessionId
    ? "Open or send a message in a session to browse its worktree."
    : "The worktree is empty.";

  function renderChanges(list: GitChange[], label: string): ReactNode {
    if (list.length === 0) return null;
    return (
      <div className="git-group">
        <div className="git-group-title">{label}</div>
        {list.map((change) => (
          <button
            key={`${change.staged ? "s" : "u"}:${change.path}`}
            type="button"
            className={`git-change ${selectedPath === change.path ? "selected" : ""}`}
            title={change.path}
            onClick={() => void selectChange(change.path)}
          >
            <span className={`git-status git-${change.status}`}>{STATUS_LABEL[change.status] ?? "M"}</span>
            <span className="git-change-path">{change.path}</span>
          </button>
        ))}
      </div>
    );
  }

  const diffPane = error ? (
    <div className="workspace-preview-empty">{error}</div>
  ) : diff ? (
    <DiffViewer diff={diff.diff} binary={diff.binary} />
  ) : (
    <div className="workspace-preview-empty">
      {view === "files" ? "Select a file to preview it." : "Select a change or a commit to see its diff."}
    </div>
  );

  return (
    <section className="chat-popover workspace-popover" style={popover.style} aria-label={title}>
      <div className="workspace-header">
        <div className="workspace-title">
          {view === "files" ? <FolderTree size={15} /> : <GitBranch size={15} />}
          <strong>{title}</strong>
          {view === "git" ? <span className="workspace-count">{changes.length}</span> : null}
        </div>
        <div className="workspace-actions">
          <button
            type="button"
            className="icon-button"
            aria-label="Refresh"
            title="Refresh"
            onClick={() => {
              if (view === "files") {
                setEntries({});
                setExpanded(new Set());
                void loadDirectory("");
              } else {
                refreshGit();
              }
            }}
          >
            <RefreshCw size={15} />
          </button>
          <button type="button" className="icon-button" aria-label="Close explorer" title="Close" onClick={onClose}>
            <X size={16} />
          </button>
        </div>
      </div>

      {view === "files" ? (
        <div className="workspace-split" ref={splitRef}>
          <div className="workspace-tree" style={{ flex: `0 0 ${splitRatio * 100}%`, minWidth: 0 }}>
            {Object.keys(entries).length === 0 && loadingDirs.size === 0 ? (
              <div className="workspace-tree-empty">{emptyHint}</div>
            ) : (
              <FileTree
                entries={entries}
                expanded={expanded}
                loading={loadingDirs}
                selectedPath={selectedPath}
                onToggleDir={toggleDir}
                onSelectFile={(path) => void selectFile(path)}
              />
            )}
          </div>
          <div
            className="workspace-divider"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize explorer"
            onPointerDown={onSplitPointerDown}
          />
          <div className="workspace-preview" style={{ flex: 1, minWidth: 0 }}>
            {error ? <div className="workspace-preview-empty">{error}</div> : null}
            {!error && file ? <FilePreview file={file} /> : null}
            {!error && !file ? <div className="workspace-preview-empty">Select a file to preview it.</div> : null}
          </div>
        </div>
      ) : (
        <div className="workspace-split" ref={splitRef}>
          <div className="workspace-preview git-column" ref={gitColumnRef} style={{ flex: `0 0 ${splitRatio * 100}%`, minWidth: 0 }}>
            <div className="git-pane git-pane-top" style={{ height: gitTopHeight }}>
              <div className="git-pane-header"><span>Changes</span></div>
              <div className="git-changes">
                {changes.length === 0 ? (
                  <div className="workspace-tree-empty">No changes in this worktree.</div>
                ) : (
                  <>
                    {renderChanges(staged, "Staged")}
                    {renderChanges(unstaged, "Changes")}
                  </>
                )}
              </div>
              <div className="git-commit">
                <textarea
                  aria-label="Commit message"
                  placeholder="Commit message"
                  value={commitMessage}
                  rows={2}
                  onChange={(event) => setCommitMessage(event.target.value)}
                />
                <div className="git-commit-actions">
                  <button
                    type="button"
                    className="approve-button"
                    disabled={gitBusy || changes.length === 0}
                    onClick={() => void commit()}
                  >
                    {gitBusy ? <Loader2 size={13} className="spin" /> : <Check size={13} />} Commit
                  </button>
                  <button type="button" className="outline-button" disabled={gitBusy} onClick={() => void push()}>
                    <Upload size={13} /> Push
                  </button>
                </div>
              </div>
            </div>
            <div
              className="git-h-divider"
              role="separator"
              aria-orientation="horizontal"
              aria-label="Resize history"
              onPointerDown={onGitSplitPointerDown}
            />
            <div className="git-pane git-pane-bottom">
              <div className="git-pane-header"><span>History</span></div>
              <div className="git-log">
                {commits.length === 0 ? (
                  <div className="workspace-tree-empty">No commits yet.</div>
                ) : (
                  commits.map((commitEntry) => (
                    <button
                      key={commitEntry.id}
                      type="button"
                      className={`git-commit-row ${selectedCommit === commitEntry.id ? "selected" : ""}`}
                      title={`${commitEntry.short} ${commitEntry.message}`}
                      onClick={() => void selectCommit(commitEntry.id)}
                    >
                      <GitCommitHorizontal size={13} />
                      <span className="git-commit-main">
                        <span className="git-commit-message">{commitEntry.message || "(no message)"}</span>
                        <span className="git-commit-meta">
                          {commitEntry.short} · {commitEntry.author}
                          {commitEntry.timestamp ? ` · ${formatWhen(commitEntry.timestamp)}` : ""}
                        </span>
                      </span>
                    </button>
                  ))
                )}
              </div>
            </div>
          </div>
          <div
            className="workspace-divider"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize diff"
            onPointerDown={onSplitPointerDown}
          />
          <div className="workspace-tree git-diff">{diffPane}</div>
        </div>
      )}
      {popover.handle}
    </section>
  );
}
