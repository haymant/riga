import type { ReactNode } from "react";
import {
  Braces,
  ChevronDown,
  ChevronRight,
  FileCode,
  FileCode2,
  FileCog,
  FileJson,
  FileText,
  FileType,
  Folder,
  FolderOpen,
  Loader2,
} from "lucide-react";
import type { WorkspaceEntry } from "./protocol";

export type FileTreeProps = {
  /** Loaded children per directory path (`""` is the workspace root). */
  entries: Record<string, WorkspaceEntry[]>;
  expanded: Set<string>;
  loading: Set<string>;
  selectedPath?: string;
  onToggleDir: (path: string) => void;
  onSelectFile: (path: string) => void;
};

/** A file-type icon + colour, echoing the VSCode explorer's type glyphs. */
function fileGlyph(name: string): { icon: ReactNode; className: string } {
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";
  switch (ext) {
    case "ts":
    case "tsx":
      return { icon: <FileCode size={15} />, className: "ft-ts" };
    case "js":
    case "jsx":
    case "mjs":
    case "cjs":
      return { icon: <FileCode2 size={15} />, className: "ft-js" };
    case "json":
      return { icon: <FileJson size={15} />, className: "ft-json" };
    case "md":
    case "mdx":
      return { icon: <FileText size={15} />, className: "ft-md" };
    case "py":
      return { icon: <FileCode size={15} />, className: "ft-py" };
    case "rs":
      return { icon: <FileCog size={15} />, className: "ft-rs" };
    case "yml":
    case "yaml":
    case "toml":
      return { icon: <Braces size={15} />, className: "ft-yml" };
    case "css":
    case "scss":
    case "html":
    case "svg":
      return { icon: <FileType size={15} />, className: "ft-web" };
    default:
      return { icon: <FileText size={15} />, className: "ft-default" };
  }
}

/**
 * A lazy file tree: a directory's children are only present in `entries` once
 * the parent has been expanded, and expanding asks the backend for that one
 * level rather than walking the whole workspace.
 */
export function FileTree({ entries, expanded, loading, selectedPath, onToggleDir, onSelectFile }: FileTreeProps) {
  function renderLevel(parent: string, depth: number): ReactNode {
    const children = entries[parent] ?? [];
    return children.map((entry) => {
      const isDir = entry.kind === "dir";
      const isOpen = isDir && expanded.has(entry.path);
      const isBusy = loading.has(entry.path);
      const glyph = fileGlyph(entry.name);
      return (
        <div key={entry.path} className="file-tree-node">
          <button
            type="button"
            className={`file-tree-row ${selectedPath === entry.path ? "selected" : ""}`}
            style={{ paddingLeft: 6 + depth * 14 }}
            title={entry.path}
            onClick={() => (isDir ? onToggleDir(entry.path) : onSelectFile(entry.path))}
          >
            <span className="file-tree-chevron">
              {isDir ? (
                isBusy ? (
                  <Loader2 size={13} className="spin" />
                ) : isOpen ? (
                  <ChevronDown size={14} />
                ) : (
                  <ChevronRight size={14} />
                )
              ) : null}
            </span>
            <span className={`file-tree-icon ${isDir ? "ft-folder" : glyph.className}`}>
              {isDir ? (isOpen ? <FolderOpen size={15} /> : <Folder size={15} />) : glyph.icon}
            </span>
            <span className="file-tree-name">{entry.name}</span>
          </button>
          {isOpen && children.length === 0 && !isBusy ? (
            <div className="file-tree-empty" style={{ paddingLeft: 6 + (depth + 1) * 14 }}>
              empty
            </div>
          ) : null}
          {isOpen ? renderLevel(entry.path, depth + 1) : null}
        </div>
      );
    });
  }

  return <div className="file-tree" role="tree">{renderLevel("", 0)}</div>;
}
