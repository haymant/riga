

## Latest interaction fixes

The comma key is now ordinary composer input; only Ctrl+Comma and F2 open settings. User and assistant labels now use distinct colors, and the message bodies use separate foreground colors as well. Starting another run archives the previous visible prompt and assistant transcript into the current session view instead of replacing the transcript with only the newest run. Raw LF and CR composer events are normalized to newline input in addition to enhanced Shift+Enter events.


## Command palette, RunDeck, and session recovery follow-up

The TUI now uses Ctrl+J for multiline input. Ctrl+D toggles a RunDeck side panel rather than navigating away from the transcript. Slash and at-command completion filters candidates as the user types: slash candidates are populated from tools, MCP/skill catalog entries, and built-in commands; at-candidates include workspace entries and catalog agents/skills.

The server now discovers session ids from durable JSON run journals under `RIGA_DATA_DIR/runs/` when the persisted session list is missing or incomplete. This covers installations where the GUI-visible activity exists as run journals but `sessions.enc`/`sessions.json` was never written.

Text styling follows the common terminal-agent convention of distinct role colors, dim reasoning, and semantic colors for code-like, JSON-like, shell, list, and enum-like assistant output. OpenCode documents ANSI palette-based syntax/UI themes and a dedicated TUI; Codex documents visible model/status context and resumable terminal sessions; Gemini documents a reason-and-act loop with tools and MCP servers; Claude documents terminal configuration for multiline input, color theme matching, and Vim mode.

References:

- https://opencode.ai/docs/tui/
- https://opencode.ai/docs/themes/
- https://learn.chatgpt.com/docs/codex/cli
- https://docs.cloud.google.com/gemini/docs/codeassist/gemini-cli
- https://code.claude.com/docs/en/terminal-config
- https://code.claude.com/docs/en/output-styles
