# RIGA CLI / TUI Test Plan

**Status:** Implementation-ready, phased

**Scope:** `crates/riga-cli`, the existing `riga_server::ipc::IpcService` adapter, and the optional WebSocket adapter required for a Ratatui UI.

**Out of scope:** Browser/Tauri pixel behavior, provider/model quality, GPU/llama performance, external MCP services, and Steer/Codex source or runtime dependencies.

## 1. Corrections to the supplied draft

The supplied draft contains good test-pyramid ideas, but these assumptions do not match the current repository and are corrected here:

1. `riga-cli` is currently a scaffold with no `tui`, `headless`, `session`, or `turn` subcommands. Tests must not assume those commands until an implementation phase adds and documents them.
2. The default TUI transport is in-process `riga_server::ipc::IpcService`, matching Tauri. The current WebSocket protocol remains an optional adapter and is not the first implementation target. Sessions, catalog, MCP registry, attachments, and local-model management already have IPC methods.
3. `ClientMessage` and `ServerMessage` are currently private to `riga-server/src/ws.rs`. Protocol extraction is therefore an implementation prerequisite, not an already-available test seam.
4. No `termlens` or `testty` dependency is present. PTY testing is deferred until the application has a stable binary and a dependency decision. It must not block deterministic unit, protocol, or `TestBackend` tests.
5. No deterministic `RIGA_TEST_PROVIDER` fixture switch currently exists. TUI transport tests should use the existing deterministic IPC test seam in `riga-server` or a CLI-owned fake transport; they must not call a live provider.
6. `RigaTransport` is a TypeScript interface, not a Rust trait currently shared with the CLI. Rust tests must target the extracted wire contract and a CLI-owned transport seam; they must not pretend a Rust implementation already exists.
7. IPC delivers typed `RigaEventEnvelope` values directly. Optional WebSocket fixtures must preserve the actual externally tagged `RigaEvent` representation instead of assuming snake-case event tags.
8. `cargo test -p riga-cli --test render` and similar commands are invalid until those integration targets exist. The phase commands below use targets that exist or are explicitly created by that phase.
9. The repository has no established `insta` snapshot dependency. Initial render tests should use `ratatui::backend::TestBackend` buffer assertions; snapshot tooling can be added later if the UI warrants it.
10. No real API key, external network, or live LLM is allowed in any automated test.

## 2. Test principles

- The CLI is an IPC client by default and an optional WebSocket client later; it is never a second agent runtime.
- Kernel/server tests own agent, policy, provider, task, evidence, and knowledge correctness.
- CLI tests own message serialization, transport behavior, event projection, input behavior, rendering, recovery, and exit behavior.
- Every test is deterministic and hermetic.
- No test logs secrets or depends on the developer’s home configuration.
- Tests must assert terminal events and sequence behavior, not merely that a socket remained open.
- A user-visible feature requires a state test and a rendering test before it is considered implemented.
- A phase may be committed only when its exit criteria and evidence are recorded below or in a phase report.

## 3. Test layers

| Layer | Purpose | Tooling | First phase |
|---|---|---|---|
| P0 | IPC transport seam and typed event flow | `cargo test`, `IpcService` deterministic seam | TUI-0 |
| P1 | Headless projection and input | Rust unit tests and fake transport | TUI-1 |
| P1b | Optional WebSocket compatibility | adapter-local fixtures or shared protocol if justified | later |
| P2 | Ratatui render/state integration | `TestBackend`, buffer assertions | TUI-2/TUI-3 |
| P3 | Real protocol integration | embedded fake server, optionally `riga-server` | TUI-1 onward |
| P4 | PTY smoke | selected PTY library after decision | TUI-7 |

P4 is intentionally not a prerequisite for the first implementation phases.

## 4. Phase gates

### TUI-0 — IPC transport seam and headless smoke

#### Entry criteria

- `riga-cli` is still a scaffold.
- `IpcService` already exposes typed health, sessions, runs, approvals, and local-model methods.
- Existing workspace tests pass before CLI changes.

#### Required tests

1. Construct `IpcService::new(ServerState::default())` without an external server.
2. CLI health reports protocol version and adapter.
3. CLI lists and creates sessions through `IpcService`.
4. Deterministic IPC run emits ordered `RigaEventEnvelope` values.
5. CLI subscribes to a run and preserves terminal events.
6. CLI cancel targets the requested run id.
7. Active-run listing preserves id, session id, and local flag.
8. Approval response forwards approval id, approved flag, and always scope.
9. No CLI output contains provider API keys.

#### Exit criteria

- Default mode uses IPC and no WebSocket dependency is required.
- `--plain` or equivalent documented headless mode is available.
- All TUI-0 tests pass with no network.
- A phase report records exact commands and event evidence.

### TUI-1 — Headless projection and optional WebSocket adapter

#### Entry criteria

- TUI-0 passes.
- App state can be driven by a CLI-owned transport trait.

#### Required tests

1. Text, reasoning, tool, approval, task, evidence, knowledge, and terminal events project without panic.
2. Duplicate sequence numbers are ignored.
3. Sequence gaps are reported and IPC resume uses the last sequence.
4. Optional WebSocket mode, if implemented, sends the exact existing message shapes.
5. IPC remains the default when no transport flag is supplied.
6. Server errors and disconnects produce bounded user-facing errors.

#### Exit criteria

- All P1 projection tests pass.
- WebSocket support is either tested or explicitly deferred; it must not block IPC.
- No WebSocket extraction to `riga-kernel` is required for this phase.

### TUI-2 — Terminal lifecycle, application state, and idle/composer UI

#### Entry criteria

- TUI-1 passes.
- The application state can be driven without a real terminal.

#### Implementation under test

- `AppState` and `App::handle_key`/event seam.
- Terminal guard and cleanup.
- Ratatui layout for top bar, transcript placeholder, composer, footer, help, and connection state.
- Multiline composer, Enter send, Shift+Enter newline, draft reset after send.

#### Required tests

1. 80×24 idle screen renders without panic.
2. 120×40 idle screen renders without panic.
3. Composer accepts Unicode and multiline input.
4. Enter emits one start command and clears the draft.
5. Shift+Enter changes only the draft.
6. Up/down history behavior is deterministic.
7. Unknown keys do not mutate state or panic.
8. Connection, disconnected, and reconnecting states are visible.
9. Terminal guard restores raw mode/alternate screen on normal and error exits where testable.
10. Render functions perform no transport or filesystem I/O.

#### Exit criteria

- P2 state/render tests pass.
- TestBackend buffer assertions cover idle, composer, disconnected, and help states.
- Terminal cleanup is tested through an injectable guard seam or documented manual smoke test.

### TUI-3 — Streaming transcript, tools, reasoning, and approvals

#### Entry criteria

- TUI-2 passes.
- `AppState::apply_event` consumes the shared event envelope.

#### Required tests

1. Text deltas append in sequence.
2. Reasoning is separate and collapsible.
3. Tool call/output/result cards preserve call id and status.
4. Approval card renders tool, task, approval id, and bounded summary.
5. Allow once, always, and deny emit correct protocol commands.
6. Approval duplicate submission is prevented.
7. Run completion/failure remains visible.
8. Transcript follows output only while the viewport is at bottom.
9. Manual scroll-up disables auto-follow and shows a new-events indicator.
10. Jump-to-bottom resumes following.
11. Long output is bounded without invalid UTF-8 slicing.

#### Exit criteria

- All three P2 streaming/approval render and state tests pass.
- A fixed-size TestBackend test exists for each new user-visible card.
- A fake protocol test proves approval commands reach the server script.

### TUI-4 — Sessions, settings, and catalog only after protocol support

#### Entry criteria

- The required operations have documented protocol messages or a deliberately supported transport adapter.
- No test relies on an undocumented HTTP route.

#### Required tests

- session list/create/select behavior;
- provider fields and optional-field defaults;
- API-key masking and no persistence/logging;
- catalog filtering and draft insertion without execution;
- explicit unsupported-state rendering for operations not yet exposed over the chosen transport.

#### Exit criteria

- Every implemented operation is in the protocol coverage matrix.
- Unsupported operations are visible and tested rather than silently omitted.

### TUI-5 — RunDeck execution/evidence/knowledge

#### Entry criteria

- TUI-3 passes.
- Canonical event projections for plan/todo/graph/task/evidence/knowledge exist.

#### Required tests

- collapsed footer summary;
- expanded execution lens;
- evidence lens empty/non-empty;
- knowledge lens empty/non-empty;
- active-run list and target-specific stop;
- graph dependency ordering and textual fallback;
- narrow-terminal overlay behavior;
- selected-run switching without cross-run state contamination.

#### Exit criteria

- All RunDeck lenses render from event projections.
- Active-run recovery and stop are tested against a fake server.
- No RunDeck widget performs orchestration or policy decisions.

### TUI-6 — Local models and attachments

#### Entry criteria

- A shared protocol operation exists for each implemented feature.
- The operation is available to the chosen server transport.

#### Required tests

- overview/catalog parsing;
- download progress/finish/failure/cancel;
- load/unload state;
- attachment capability or explicit unsupported state;
- no large binary fixture committed unnecessarily.

#### Exit criteria

- No TUI-only endpoint or JSON shape.
- All progress state tests are deterministic.

### TUI-7 — PTY and release hardening

#### Entry criteria

- TUI-2 through TUI-5 pass.
- The binary has a documented interactive startup command.
- A PTY library is selected after checking workspace compatibility.

#### Required tests

Keep the initial PTY suite to four journeys:

1. start and clean quit;
2. composer submit against scripted server;
3. approval allow/deny;
4. resize from 80×24 to 100×30.

Use wait-until-screen predicates, not arbitrary sleeps. Save failure screens under `target/test-artifacts/pty/`.

#### Exit criteria

- PTY tests pass locally in a fixed environment.
- Terminal restoration is demonstrated after normal quit and injected failure.
- Full workspace test and lint gate passes.

## 5. Test data and security rules

- Use fixed session/run ids in fixtures.
- Redact timestamps and absolute paths in render output.
- Never place an API key in a fixture; use `<redacted>` only.
- Use `TempDir` for data and workspace tests.
- Set `RIGA_DATA_DIR` to a temporary path for tests that touch persistence.
- Use `127.0.0.1:0` for fake servers.
- Do not call public APIs, model providers, MCP services, or the network outside the local fake server.
- Do not commit `references/steer` as a source dependency or test fixture.

## 6. Failure artifacts

When a test fails, retain only deterministic diagnostics:

```text
target/test-artifacts/
  protocol/<test-name>.json
  pty/<test-name>.screen.txt
  render/<test-name>.txt
```

Diagnostics must redact API keys, authorization headers, home paths, and full prompts when they contain secrets.

## 7. Required phase report format

Each completed phase should add a short report at `docs/BuildPlan-TUI/phase-<n>-report.md` containing:

```markdown
# TUI Phase <n> Report

Status: PASS | BLOCKED
Entry criteria:
Exit criteria:
Files changed:
Protocol changes:
Tests run:
Evidence:
Known limitations:
Next phase:
```

Evidence must include exact commands and pass counts, not only a claim that tests passed.

## 8. Current first execution sequence

The coding agent must follow this order now:

1. Write this test plan.
2. Implement TUI-0 IPC transport seam and headless smoke.
3. Run TUI-0 commands and fix all failures.
4. Record `phase-0-report.md`.
5. Commit and push TUI-0 only after its exit criteria pass.
6. Implement TUI-1 headless projection; add optional WebSocket only if justified.
7. Run TUI-1 commands and fix all failures.
8. Record and commit TUI-1 only after its exit criteria pass.
9. Continue with TUI-2 onward only after the preceding phase is green.

Do not claim full TUI completion merely because TUI-0 or TUI-1 passes.
