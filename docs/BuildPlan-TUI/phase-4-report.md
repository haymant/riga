# TUI Phase 4 Report

Status: **PASS**

TUI-4 adds protocol-backed session history, provider settings, and a server catalog command palette without creating a second runtime or a second tool registry.

## Implemented behavior

The history panel opens with `Ctrl+H`, supports selection with Up/Down and Enter, creates validated sessions with `n`, and preserves the main draft while it is open. Session selection updates the selected session and clears the active run scope; an empty session list has an explicit empty state.

Provider settings open with `Ctrl+,`. The form includes endpoint, masked API key, model, reasoning effort, provider kind, provider API, and optional subagent model. Tab and Shift+Tab move between fields; Enter cycles the enumerated fields; `Ctrl+S` is the explicit save action; and Esc closes without saving. API keys are kept in memory only on the CLI side, are never rendered, and are passed only through `IpcService::configure_provider`, which persists them through the server secure-store path. Existing server configuration is loaded without returning the key.

The catalog palette opens with `Ctrl+K`. It parses the server-provided catalog, displays kind, description, and approval metadata, filters incrementally, and inserts the server-provided `insert_text` into the draft. Selecting a catalog item never executes a tool. `?` opens the keyboard help panel.

## Files changed

- `crates/riga-cli/src/app.rs`
- `crates/riga-cli/src/main.rs`
- `crates/riga-cli/src/model.rs`
- `crates/riga-cli/src/transport.rs`
- `crates/riga-cli/src/ui.rs`
- `crates/riga-server/src/ws.rs`
- `docs/BuildPlan-TUI/phase-4-report.md`

## Protocol and security

No new wire protocol was introduced. TUI-4 uses the existing `IpcService::list_sessions`, `create_session`, `catalog`, `provider`, and `configure_provider` operations. Provider configuration commands use the shared `RigaTransport` seam. The API key is not returned by the provider-read operation, is masked in the settings renderer, and is absent from tests and reports.

## Validation

```text
cargo fmt --all
cargo test -p riga-cli
```

The focused suite passes with **19 tests, 0 failures**, including session creation without draft loss, provider save/key handling, catalog filtering/insertion, and TestBackend API-key masking.

The full workspace validation gate remains the next required check before merging this phase.

## Known limitations

The optional WebSocket adapter is not added to the CLI transport seam. MCP registry editing, local-model controls, attachments, and RunDeck lenses remain assigned to later phases. Catalog entries that do not expose a server `insert_text` are display-only, so the TUI does not invent executable behavior for them.

## Next phase

TUI-5 is allowed: RunDeck parity, active-run recovery, plan/todo/task/graph views, evidence and knowledge lenses, and narrow-terminal fallbacks.
