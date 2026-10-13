export type NativeSlashCommand = {
  name: string;
  description: string;
};

export type ConversationExportItem = {
  role: "user" | "assistant" | "system" | "reasoning" | "tool";
  text?: string;
  callId?: string;
  name?: string;
  command?: string;
  output?: string;
  status?: "running" | "done" | "error";
};

/** Keep in parity with riga-cli::completion_candidates and handle_slash_command. */
export const NATIVE_SLASH_COMMANDS: readonly NativeSlashCommand[] = [
  { name: "/model", description: "Choose or set the active provider model" },
  { name: "/models", description: "Alias for /model" },
  { name: "/export", description: "Export the current conversation as JSONL" },
  { name: "/tools", description: "Browse built-in tools" },
  { name: "/mcp", description: "Browse configured MCP servers and tools" },
  { name: "/skills", description: "Browse discovered skills" },
  { name: "/settings", description: "Open provider settings" },
  { name: "/local-models", description: "Open the local model manager" },
  { name: "/new", description: "Start a new session" },
  { name: "/clear", description: "Alias for /new; keep the old session in history" },
  { name: "/resume", description: "Open session history" },
  { name: "/sessions", description: "Alias for /resume" },
  { name: "/rename", description: "Rename the current session" },
  { name: "/status", description: "Show connection, model, and session status" },
  { name: "/help", description: "Show command help" },
  { name: "/exit", description: "Exit RIGA" },
  { name: "/quit", description: "Alias for /exit" },
];

export function matchingNativeSlashCommands(query: string): NativeSlashCommand[] {
  const normalized = query.toLowerCase().replace(/^\/+/, "");
  return NATIVE_SLASH_COMMANDS.filter((command) => command.name.slice(1).startsWith(normalized));
}

export function parseNativeSlashCommand(input: string): { name: string; arguments: string } | null {
  const trimmed = input.trim();
  if (!trimmed.startsWith("/")) return null;
  const [rawName, ...parts] = trimmed.split(/\s+/u);
  if (!rawName) return null;
  return { name: rawName.toLowerCase(), arguments: parts.join(" ") };
}

export function conversationJsonl(sessionId: string | null, sessionTitle: string | null, items: ConversationExportItem[]): string {
  const records: Record<string, unknown>[] = [{
    type: "conversation",
    format: "riga-jsonl-v1",
    session_id: sessionId,
    session_title: sessionTitle,
  }];
  for (const item of items) {
    if (item.role === "tool") {
      records.push({ type: "tool_call", call_id: item.callId ?? null, tool: item.name ?? "", arguments: item.command ?? "" });
      if (item.status !== "running") records.push({ type: "tool_output", call_id: item.callId ?? null, output: item.output ?? "" });
    } else if (item.role === "reasoning") {
      records.push({ type: "reasoning", content: item.text ?? "" });
    } else {
      records.push({ type: "message", role: item.role, content: item.text ?? "" });
    }
  }
  return `${records.map((record) => JSON.stringify(record)).join("\n")}\n`;
}

export function conversationExportFilename(argument: string, fallback: string): string {
  const requested = argument.replace(/^["']|["']$/gu, "").trim();
  const baseName = requested.split(/[\\/]/u).filter(Boolean).at(-1) || fallback;
  return baseName.toLowerCase().endsWith(".jsonl") ? baseName : `${baseName}.jsonl`;
}
