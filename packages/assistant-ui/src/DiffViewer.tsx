import { useMemo } from "react";

function lineClass(line: string): string {
  if (line.startsWith("@@")) return "diff-line hunk";
  if (line.startsWith("+++") || line.startsWith("---")) return "diff-line header";
  if (
    line.startsWith("diff --git") ||
    line.startsWith("index ") ||
    line.startsWith("new file") ||
    line.startsWith("deleted file") ||
    line.startsWith("similarity ") ||
    line.startsWith("rename ")
  ) {
    return "diff-line meta";
  }
  if (line.startsWith("+")) return "diff-line add";
  if (line.startsWith("-")) return "diff-line remove";
  return "diff-line context";
}

/**
 * A unified-diff viewer: added/removed lines are tinted, hunks and file headers
 * are separated out. It renders text only, so a diff of a generated file is
 * safe to show.
 */
export function DiffViewer({ diff, binary }: { diff: string; binary?: boolean }) {
  const lines = useMemo(() => diff.replace(/\n$/, "").split("\n"), [diff]);
  if (binary) {
    return <p className="diff-empty">Binary file — no text diff to show.</p>;
  }
  if (!diff.trim()) {
    return <p className="diff-empty">No changes to show for this file.</p>;
  }
  return (
    <div className="diff-viewer">
      {lines.map((line, index) => (
        <pre key={index} className={lineClass(line)}>
          {line.length === 0 ? " " : line}
        </pre>
      ))}
    </div>
  );
}
