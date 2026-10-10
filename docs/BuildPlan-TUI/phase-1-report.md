# TUI Phase 1 Report

Status: **COMPLETE**

## Scope

TUI-1 delivers the headless application projection boundary required before Ratatui rendering. The implementation is transport-neutral at the application seam and keeps IPC as the default runtime adapter. WebSocket support is explicitly deferred; it is optional and does not block IPC-first progress.

## Files changed

- `crates/riga-cli/src/model.rs`
- `crates/riga-cli/src/transport.rs`
- `crates/riga-cli/src/main.rs`
- `docs/BuildPlan-TUI/phase-1-report.md`

## Implemented behavior

`AppState::apply_event` consumes canonical `RigaEventEnvelope` values directly. It validates the protocol version, ignores duplicate sequence numbers, reports sequence gaps and marks the affected run as needing replay, keeps compact projections for non-selected runs, and preserves the last event cursor per run.

`RunView` projects text, reasoning, tool calls, bounded tool output, tool results, plans, todos, graphs, task lifecycle, approvals, evidence, knowledge, and terminal run state. Reasoning and tool output remain distinct transcript items rather than being flattened into assistant text. Terminal events remain visible and clear pending approvals. Transcript items, reasoning, tool output, evidence, and knowledge have explicit caps to prevent an unbounded stream from exhausting the TUI process.

`RigaTransport` is a CLI-owned async semantic seam. `IpcTransport` implements it without changing the existing `IpcService` behavior, so a future WebSocket adapter can be added without changing the projection or renderer.

## Validation evidence

Executed with the repository Rust toolchain loaded from `$HOME/.cargo/env`:

```text
cargo fmt --all
cargo test -p riga-cli
cargo clippy -p riga-cli --all-targets -- -D warnings
```

Results:

- Formatting passed.
- `cargo test -p riga-cli`: **6 passed, 0 failed**.
- `cargo clippy -p riga-cli --all-targets -- -D warnings`: passed with no warnings.

The six tests cover streaming text/reasoning/tools, duplicate and gap sequence handling, approval and terminal-state behavior, protocol-version rejection, and the existing IPC transport seam. No network, API key, provider, or external server is used.

## Explicit deferral

The optional WebSocket adapter is not implemented in this phase. No WebSocket extraction into `riga-kernel` is required. The future adapter must preserve the existing server message shapes and implement `RigaTransport`; it must not introduce a TUI-only protocol.

## Exit criteria

All P1 projection tests pass, IPC remains the default transport, and the WebSocket decision is documented as an explicit non-blocking deferral. The next phase is TUI-2: terminal lifecycle, application event loop, Ratatui rendering, and idle/composer UI.
