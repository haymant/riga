import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { conversationExportFilename, conversationJsonl, NATIVE_SLASH_COMMANDS, matchingNativeSlashCommands, parseNativeSlashCommand } from "./native-commands";

const repoFile = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");

function rustCommandNames(source: string): string[] {
  return Array.from(source.matchAll(/\("(\/[^"]+)",\s*"/gu), (match) => match[1] ?? "").sort();
}

describe("native slash command parity", () => {
  it("uses the same command names in GUI suggestions, TUI suggestions, and TUI dispatch", () => {
    const guiNames = NATIVE_SLASH_COMMANDS.map((command) => command.name).sort();
    const tuiMain = repoFile("../../../crates/riga-cli/src/main.rs");
    const tuiApp = repoFile("../../../crates/riga-cli/src/app.rs");
    const suggestionStart = tuiMain.indexOf("for (command, detail) in [");
    const suggestionEnd = tuiMain.indexOf("] {", suggestionStart);
    const dispatcherStart = tuiApp.indexOf("fn is_native_slash_command");
    const dispatcherEnd = tuiApp.indexOf("\n}", dispatcherStart);
    expect(suggestionStart).toBeGreaterThanOrEqual(0);
    expect(dispatcherStart).toBeGreaterThanOrEqual(0);
    const tuiSuggestions = rustCommandNames(tuiMain.slice(suggestionStart, suggestionEnd));
    const tuiDispatcher = Array.from(
      tuiApp.slice(dispatcherStart, dispatcherEnd).matchAll(/"(\/[^"]+)"/gu),
      (match) => match[1] ?? "",
    ).sort();
    expect(guiNames).toEqual(tuiSuggestions);
    expect(guiNames).toEqual(tuiDispatcher);
  });

  it("filters commands by prefix and parses arguments case-insensitively", () => {
    expect(matchingNativeSlashCommands("/mod").map((command) => command.name)).toEqual(["/model", "/models"]);
    expect(parseNativeSlashCommand("/MODEL set openai/gpt-4.1")).toEqual({
      name: "/model",
      arguments: "set openai/gpt-4.1",
    });
    expect(parseNativeSlashCommand("explain /model in this sentence")).toBeNull();
  });

  it("exports only the supplied session transcript in the shared JSONL-v1 format", () => {
    const lines = conversationJsonl("session-current", "Current chat", [
      { role: "user", text: "Question" },
      { role: "assistant", text: "Answer" },
    ]).trimEnd().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>);
    expect(lines).toEqual([
      { type: "conversation", format: "riga-jsonl-v1", session_id: "session-current", session_title: "Current chat" },
      { type: "message", role: "user", content: "Question" },
      { type: "message", role: "assistant", content: "Answer" },
    ]);
    expect(conversationExportFilename("exports/today", "fallback.jsonl")).toBe("today.jsonl");
  });
});
