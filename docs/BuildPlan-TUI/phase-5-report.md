# TUI Phase 5 Report

Status: **PASS**

TUI-5 adds the RunDeck command center over the existing canonical run projection. It does not create a second task store or infer task policy in the CLI.

## Implemented behavior

`Ctrl+D` opens RunDeck. The Execution, Evidence, and Knowledge lenses are selected with Tab or Left/Right. Execution renders the current plan, todo progress, task queue/state, graph dependencies, run status, sequence, and progress ratio. Evidence and Knowledge render their canonical event-backed cards with source and confidence metadata.

RunDeck lists active runs with run id, session id, and local/remote state. Enter resumes the selected run through the existing subscription path; `x` stops the selected run through the existing cancellation path. The CLI loads active runs on startup and updates the list when it starts or stops a run. A run is never considered active merely because it appears in the session list.

The panel remains keyboard-only and uses plain dependency-list graph fallback, so it remains usable without a mouse or canvas. Empty active-run, evidence, and knowledge states are explicit.

## Files changed

- `README.md`
- `crates/riga-cli/src/app.rs`
- `crates/riga-cli/src/main.rs`
- `crates/riga-cli/src/model.rs`
- `crates/riga-cli/src/ui.rs`
- `crates/riga-server/src/ws.rs`
- `docs/BuildPlan-TUI/BuildPlan-TUI.md`
- `docs/BuildPlan-TUI/phase-5-report.md`

## Protocol changes

No new wire protocol was introduced. Active-run recovery uses the existing IPC active-run listing, subscription, and cancellation operations. `ActiveRun` identifiers are exposed as public adapter data so thin CLI rendering and control code can consume them.

## Validation

```text
cargo fmt --all -- --check
cargo test -p riga-cli
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The focused CLI suite passes with **20 tests, 0 failures**. The workspace gate passes with clippy clean, 20 CLI tests, 33 kernel tests, 120 server tests, 3 shell tests, and all doctests passing.

## Known limitations

The graph is rendered as a keyboard-readable dependency list rather than a mouse canvas. Child tool-call expansion, retrying a failed graph node, focused-node evidence filtering, local-model controls, and attachments remain later work. The optional WebSocket CLI adapter remains deferred.

## Next phase

TUI-6 is allowed: local-model and attachment parity, with explicit unsupported-state handling wherever the shared protocol does not yet expose an operation.
