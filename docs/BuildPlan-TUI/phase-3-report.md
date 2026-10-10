# TUI Phase 3 Report

Status: **PASS**

## Entry criteria

- TUI-2 terminal lifecycle and TestBackend rendering pass.
- `AppState::apply_event` consumes the canonical `RigaEventEnvelope`.

## Exit criteria

- Text, reasoning, tool, approval, task, and terminal events remain distinct in the projection.
- Streaming output is bounded and UTF-8 safe.
- Approval actions dispatch through the CLI transport seam with once, always, and deny semantics.
- Duplicate approval submissions are blocked while a decision is in flight.
- Transcript follow mode, manual scroll-up, new-event indication, and jump-to-bottom are implemented.
- Reasoning and tool details can be collapsed without flattening them into assistant text.
- Completion and failure remain visible, with unresolved approvals marked terminally denied on failure/cancellation.

## Files changed

- `crates/riga-cli/src/app.rs`
- `crates/riga-cli/src/main.rs`
- `crates/riga-cli/src/model.rs`
- `crates/riga-cli/src/transport.rs`
- `crates/riga-cli/src/ui.rs`
- `docs/BuildPlan-TUI/BuildPlan-TUI.md`
- `docs/BuildPlan-TUI/TEST.md`
- `docs/BuildPlan-TUI/phase-0-report.md`
- `docs/BuildPlan-TUI/phase-3-report.md`

## Protocol changes

No wire protocol or kernel event changes. Approval decisions use the existing `IpcService::respond_to_approval` path and preserve the server-owned policy boundary.

## Tests run

```text
cargo fmt --all
cargo test -p riga-cli
```

Results:

- `cargo fmt --all`: passed.
- `cargo test -p riga-cli`: **15 passed, 0 failed**.

## Evidence

- Projection tests cover ordered text/reasoning/tool events, duplicate and gap sequences, protocol mismatch, terminal events, approval resolution, and bounded Unicode-safe streams.
- Application tests cover Enter-to-start, approval once/always/deny command mapping, duplicate submission prevention, scroll pause, and jump-to-bottom.
- Render tests cover 80×24 and 120×40 layouts plus disconnected-state visibility.
- The clean rewritten history contains zero tracked `references/steer` paths; the branch was force-pushed before TUI-3 work resumed.

## Known limitations

- The optional WebSocket transport remains deferred.
- The current TUI does not yet provide session history, provider settings, catalog insertion, RunDeck lenses, local-model controls, attachments, or PTY coverage.
- Approval cards are currently surfaced in the transcript/composer area rather than as a separate modal widget.
- Manual interactive smoke requires a terminal with an existing configured RIGA session/provider.

## Next phase

TUI-4 is allowed: sessions, settings, catalog, and explicit unsupported-state handling for operations not yet exposed through the selected transport.
