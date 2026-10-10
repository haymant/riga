# RIGA TUI BuildPlan

## Status

**Active phased implementation.** This document specifies how to extend `crates/riga-cli` into a full terminal UI that provides feature parity with `@rigai/assistant-ui` while using the same RIGA protocol and runtime semantics. TUI-0 (IPC headless transport), TUI-1 (headless projection), TUI-2 (terminal lifecycle), TUI-3 (streaming transcript and approvals), and TUI-4 (sessions, settings, and catalog) are complete; later phases remain implementation work.

The Ratatui renderer is implemented through TUI-3 and continues to expand in later feature phases. Current implementation reports live under `docs/BuildPlan-TUI/`.

## Mandatory provenance and licensing boundary

The TUI is authored solely from RIGA requirements, protocol contracts, and the existing kernel/server implementation.

Do not copy, adapt, translate, port, or mechanically reproduce code, module structure, identifiers, comments, tests, styling, or algorithms from external TUI implementations. Do not add external TUI dependencies. The resulting implementation must be authored from RIGA requirements and the RIGA protocol.

Allowed reference use:

- Observe general terminal interaction patterns such as a persistent input area, scrollable transcript, approval interruption, session switching, command palette, and explicit status surfaces.
- Observe general Ratatui layout techniques from public documentation.


Not allowed:


- Reusing their protocol, event model, tool schema, session database, prompt format, or approval semantics.
- Making RIGA behavior depend on a reference project.


RIGA remains the sole source of truth for behavior. The TUI is a second presentation variant of the same RIGA protocol: React in `@rigai/assistant-ui`, Ratatui in `riga-cli`.

---

## 1. Product objective

Create a terminal-native RIGA client with the same user-visible capabilities as the current assistant-ui application:

1. Use the existing in-process `riga_server::ipc::IpcService` by default, with an optional WebSocket adapter later.
2. Complete the RIGA handshake and expose protocol/version failures clearly.
3. List, create, select, and switch sessions.
4. Configure remote or local providers without leaking API keys.
5. Start a run with a prompt.
6. Stream text and reasoning deltas in order.
7. Render tool calls, tool output, tool results, failures, and approvals.
8. Render plan, todo, execution graph, subagents, evidence, and knowledge state.
9. Show active runs after reconnect and stop a selected run.
10. Resume a run from an event sequence cursor after reconnect.
11. Approve or deny gated tool calls, including once and always/session scope.
12. Browse composer insert items: tools, skills, agents, connectors, and workspace files.
13. Browse and load local GGUF models, with download progress and cancellation.
14. Upload attachments only if the chosen RIGA protocol transport supports the operation; otherwise show a deliberate unsupported state rather than inventing a separate upload protocol.
15. Remain usable in small terminal dimensions and degrade gracefully when optional graphics are unavailable.
16. Preserve protocol parity with the React and Tauri variants through shared fixtures and conformance tests.

The TUI must not reimplement the agent loop, tool policy, provider calls, graph readiness, evidence derivation, knowledge derivation, or task orchestration. Those remain server/kernel responsibilities.

---

## 2. Current repository facts

### 2.1 Current CLI boundary

`crates/riga-cli/src/main.rs` currently prints:

```text
RIGA CLI scaffold; implementation begins in the transport phases.
```

`crates/riga-cli/Cargo.toml` currently depends only on `riga-kernel` and the executable is a scaffold. The first implementation adds Ratatui, Crossterm, Tokio, and the existing `riga-server` IPC surface. WebSocket dependencies are optional and must not be required by default.

### 2.2 Existing protocol authority

The canonical event types are in:

- `crates/riga-kernel/src/events.rs`
- `crates/riga-kernel/src/task.rs`
- `crates/riga-kernel/src/policy.rs`

The event envelope is:

```rust
RigaEventEnvelope {
    protocol_version: u16,
    event_id: String,
    session_id: String,
    run_id: String,
    sequence: u64,
    timestamp: String,
    event: RigaEvent,
}
```

The canonical event variants include:

- `RunStarted`
- `TextDelta`
- `ReasoningDelta`
- `ToolCallStarted`
- `ToolOutputDelta`
- `ToolResult`
- `PlanUpdated`
- `TodoUpdated`
- `GraphUpdated`
- `TaskDependencyAdded`
- `TaskBlocked`
- `TaskRunnable`
- `EvidenceAdded`
- `EvidenceLinked`
- `KnowledgeCreated`
- `KnowledgeLinked`
- `TaskStarted`
- `TaskStatus`
- `TaskCompleted`
- `ApprovalRequested`
- `ApprovalResolved`
- `RunCompleted`
- `RunFailed`

The canonical task states are in `crates/riga-kernel/src/task.rs`:

- `Pending`
- `Running`
- `WaitingForApproval`
- `Completed`
- `Failed`
- `Cancelled`

The canonical policy vocabulary is in `crates/riga-kernel/src/policy.rs`:

- `ToolRisk`
- `ToolRequest`
- `ApprovalDecision`
- `ToolPolicy`

Do not create TUI-specific replacements for these domain types.

### 2.3 Existing WebSocket protocol

The server wire types currently live privately in `crates/riga-server/src/ws.rs`:

`ClientMessage` currently supports:

- `hello { client_version }`
- `configure_provider`
- `start_run { run_id, session_id, prompt }`
- `resume_run { run_id, after_sequence }`
- `cancel_run { run_id }`
- `list_active_runs`
- `approval { run_id, approval_id, approved, option }`
- `ping { nonce }`
- internal `tool_call` handling

`ServerMessage` currently supports:

- `ready { protocol_version, server_version }`
- `provider_configured`
- `event { envelope }`
- `run_cancelled`
- `active_runs { runs }`
- `approval_recorded`
- `pong`
- `error { code, message }`
- `tool_result`

Before implementing the CLI, extract or expose the public transport message contract from `riga-server/src/ws.rs` into a protocol-owned module. The server and CLI must not each silently maintain incompatible private copies.

### 2.4 Existing React transport parity surface

The TypeScript contract is in `packages/assistant-ui/src/protocol/index.ts` and defines `RigaTransport` methods for:

- connection lifecycle;
- health and catalog;
- sessions;
- provider configuration;
- start/resume/cancel run;
- active-run listing;
- approvals;
- MCP registry;
- attachments;
- local-model overview, events, downloads, cancellation, loading, unloading.

The TUI must implement the same semantic surface. It may use a smaller first milestone, but every intentionally unsupported operation must be tracked in the UI and phase report.

### 2.5 Existing product feature contracts

The feature requirements are documented in:

- `docs/Features/RUNDECK.md`
- `docs/Features/SUBAGENT.md`
- `docs/Features/EVIDENCE.md`
- `docs/Features/KNOWLEDGE.md`

Read all four before coding. They are more authoritative than screenshots or a reference TUI.

---

## 3. Architecture decision

### 3.1 Two UI variants, one protocol

```text
                         ┌─────────────────────┐
                         │   RIGA kernel        │
                         │ events/tasks/policy  │
                         └──────────┬──────────┘
                                    │
                         ┌──────────▼──────────┐
                         │   RIGA server        │
                         │ provider/agent/tools │
                         └──────────┬──────────┘
                                    │ RIGA runtime semantics / IPC
                                    │ optional WebSocket adapter
                    ┌───────────────┴────────────────┐
                    │                                │
          ┌─────────▼─────────┐            ┌─────────▼─────────┐
          │ assistant-ui       │            │ riga-cli TUI       │
          │ React/Tauri        │            │ Ratatui            │
          └────────────────────┘            └────────────────────┘
```

The TUI is a client. It must not call provider APIs directly.

The default TUI may depend on the public `riga_server::ipc::IpcService`, because this is the existing in-process adapter used by Tauri. It must not reach into private server implementation details. An optional WebSocket adapter may use the existing server wire types initially; extracting those types is a later decision, not a prerequisite for IPC.

### 3.2 Recommended transport layering

Create a transport-neutral application layer with IPC as the default:

```text
riga-cli/src/main.rs
    argument parsing and process exit policy
riga-cli/src/app.rs
    event-driven application state and command dispatch
riga-cli/src/transport/mod.rs
    transport trait and common command/event types
riga-cli/src/transport/ipc.rs
    IpcService adapter; default mode
riga-cli/src/transport/websocket.rs
    optional adapter for a running server; later phase
riga-cli/src/ui.rs
    Ratatui rendering only; no protocol I/O and no agent logic
```

Supporting modules should remain narrow:

```text
riga-cli/src/model.rs          UI projection state, not a second domain model
riga-cli/src/input.rs          key maps, command palette, text editing
riga-cli/src/theme.rs          semantic styles and terminal capability fallback
riga-cli/src/persistence.rs   local UI preferences only; never agent truth
riga-cli/src/format.rs        markdown/plain-text and bounded output formatting
riga-cli/src/error.rs         typed client/UI errors and user-facing messages
```

The IPC adapter calls `IpcService` methods such as `start_run`, `subscribe_run`,
`cancel_run`, `list_active_runs`, `respond_to_approval`, session methods, provider
methods, catalog methods, and local-model methods. It receives typed
`RigaEventEnvelope` values directly through `tokio::sync::broadcast`.

Do not put rendering code in a transport adapter. Do not put provider calls or
agent-loop logic in `riga-cli`. Do not make `RigaEvent` depend on Ratatui.

### 3.3 Runtime model

Use one async runtime and one UI thread/task boundary:

- The transport task owns the selected adapter. IPC calls `IpcService` directly; the optional WebSocket adapter owns its socket and decoding.
- The UI/application loop owns mutable `AppState` and is the only writer to it.
- User commands are sent from the UI loop to the transport adapter through a command channel or direct async service call, with one application-owned event channel.
- The render loop ticks at a bounded cadence, recommended 30–60 ms while active and 250 ms while idle.
- Streaming events must not block terminal rendering indefinitely.
- The channel must have an explicit overflow policy. Event sequence and terminal events cannot be silently dropped.

Recommended internal message types:

```rust
enum TransportEvent {
    Connected { protocol_version: u16, server_version: String },
    Disconnected { reason: String },
    Server(ServerMessage),
    ProtocolError { message: String },
}

enum UserCommand {
    StartRun { session_id: String, prompt: String },
    ResumeRun { run_id: String, after_sequence: u64 },
    CancelRun { run_id: String },
    Approve { run_id: String, approval_id: String, always: bool },
    Deny { run_id: String, approval_id: String },
    SelectSession(String),
    CreateSession { title: String, workspace: String },
    ConfigureProvider(ProviderConfigInput),
    ListActiveRuns,
    Ping,
    Shutdown,
}
```

Use `riga_kernel::events::RigaEventEnvelope` for decoded event payloads. Avoid converting it to a string and reparsing it in the UI.

---

## 4. IPC-first transport contract

The first implementation uses the existing `riga_server::ipc::IpcService` as the
default TUI transport. This is the same runtime boundary used by the Tauri
application and avoids a second server process, loopback networking, and duplicate
wire serialization.

The IPC adapter must expose a CLI-owned transport trait over these existing methods:

- `health`, `catalog`;
- `list_sessions`, `create_session`;
- `provider`, `configure_provider`;
- `start_run`, `subscribe_run`, `cancel_run`, `list_active_runs`;
- `respond_to_approval`;
- MCP registry operations;
- local-model operations;
- attachment upload.

The adapter must preserve the typed `RigaEventEnvelope` and must not reimplement
server behavior. The `riga-server` dependency is acceptable because `IpcService`
is the public in-process adapter; the CLI must not access private fields or
private helper functions.

An optional WebSocket adapter may be added after IPC parity is stable. It may use
adapter-local wire types initially. A shared protocol crate or kernel extraction
is justified only when the WebSocket adapter is implemented and serialization
drift becomes a demonstrated maintenance problem.

## 4. Protocol extraction and compatibility work

This must be the first implementation phase because the current server wire enums are private.

### 4.1 Files to modify

#### `crates/riga-kernel/src/lib.rs`

- Export the new public protocol module.
- Preserve existing `PROTOCOL_VERSION` exports.
- Do not make kernel depend on `tokio`, `axum`, Ratatui, or WebSocket crates.

#### `crates/riga-kernel/src/protocol.rs` — new

Move or define public, transport-neutral serializable message contracts:

- `ClientMessage`
- `ServerMessage`
- `ActiveRun`
- `ProviderConfigWire`
- `ProviderConfiguredWire`
- `ApprovalWire`
- `HealthWire` if health becomes a WebSocket request
- protocol version constants

Use the exact JSON tags already accepted by `riga-server/src/ws.rs`:

```rust
#[serde(tag = "type", rename_all = "snake_case")]
```

Do not change the JSON shape without updating the React transport and conformance fixtures.

The `event` member must use the canonical `RigaEventEnvelope` rather than a second event enum.

Add serde round-trip tests for every message variant and compatibility tests for optional fields (`kind`, `api`, `subagent_model`, `option`, `after_sequence`).

#### `crates/riga-server/src/ws.rs`

- Import the public protocol message types.
- Remove duplicate private definitions only after equivalent tests pass.
- Keep WebSocket-specific handling, connection ownership, approval broker, run registry, journal replay, and server behavior here.
- Preserve backward-compatible parsing of omitted optional fields.
- Keep API keys out of serialized provider-configured events and errors.

#### `packages/assistant-ui/src/protocol/index.ts`

- Document the protocol version and message fixture source.
- If generated bindings are introduced, keep the TypeScript public surface stable.
- Add a conformance fixture test that compares representative Rust JSON fixtures with TypeScript parsing.

#### `packages/assistant-ui/src/protocol/conformance.test.ts`

- Extend existing conformance coverage for any extracted message variants.
- Assert that React and TUI use the same event variant names and field semantics.

#### `crates/riga-cli/Cargo.toml`

Add only the dependencies required for a protocol client and TUI. Recommended starting set:

```toml
clap = { version = "4", features = ["derive", "env"] }
crossterm = "0.29"
futures-util = "0.3"
ratatui = "0.29"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "time", "net"] }
tokio-tungstenite = { version = "0.27", features = ["rustls-tls-native-roots"] }
uuid = { version = "1", features = ["v4"] }
```



If TLS feature selection differs in the workspace, choose a documented deterministic feature set and test both `ws://` and `wss://` parsing without putting credentials in logs.

---

## 5. CLI command and startup contract

### 5.1 `crates/riga-cli/src/main.rs`

Replace the scaffold with a small executable that:

1. parses CLI arguments;
2. initializes logging to stderr only;
3. loads local UI preferences without treating them as server state;
4. creates the `RigaTransportClient`;
5. starts the `App`;
6. restores the terminal on every exit path, including panic where practical;
7. returns a non-zero exit code for startup/protocol failures;
8. never prints secrets.

Recommended arguments:

```text
riga-cli [OPTIONS]

--server <URL>              default ws://127.0.0.1:8787/ws
--session <ID>              select an existing session
--workspace <PATH>          workspace for a new session
--prompt <TEXT>             optional one-shot prompt before interactive mode
--provider <remote|local>   optional provider-kind override
--model <MODEL>             optional model override
--no-color                  disable color styling
--plain                     disable Ratatui and print a bounded event summary
--resume <RUN_ID>           resume a known run
--after-sequence <N>        replay events after sequence N
--log-level <LEVEL>         stderr diagnostics only
```

Do not make `--prompt` bypass the TUI by default. It should start the run and keep the TUI available to inspect the stream unless `--plain` is supplied.

### 5.2 Server URL and local server behavior

The first milestone connects to an already-running RIGA server through WebSocket. Do not invent a second local provider path.

If a future `--spawn-server` option is desired, make it a separate phase that:

- starts the existing `riga-server` executable as a child process;
- waits for the existing health/handshake path;
- connects through the same WebSocket protocol;
- forwards and cleans up the child process;
- never embeds provider logic in `riga-cli`.

Do not implement server spawning in the first TUI phase unless the protocol client is already stable.

### 5.3 Terminal lifecycle

Create `TerminalGuard` in `crates/riga-cli/src/terminal.rs` or keep it in `main.rs` only if tests remain readable. It must:

- enable raw mode;
- enter the alternate screen;
- enable mouse capture only if implemented and tested;
- hide/show cursor consistently;
- restore raw mode and alternate screen on normal shutdown;
- restore terminal state on connection failure and panic hook;
- avoid writing user-visible diagnostics into the alternate screen after teardown.

All terminal writes must go through Ratatui/Crossterm ownership. Do not mix `println!` into the active screen.

---

## 6. Application state model

### 6.1 `crates/riga-cli/src/model.rs`

Create a projection model that is intentionally separate from kernel domain truth but stores canonical values, not invented semantics.

Recommended top-level state:

```rust
pub struct AppState {
    pub connection: ConnectionState,
    pub protocol_version: Option<u16>,
    pub server_version: Option<String>,
    pub sessions: Vec<SessionView>,
    pub selected_session: Option<String>,
    pub active_run: Option<String>,
    pub runs: BTreeMap<String, RunView>,
    pub active_runs: Vec<ActiveRun>,
    pub provider: ProviderView,
    pub catalog: CatalogView,
    pub transcript: Vec<TranscriptItem>,
    pub draft: TextBuffer,
    pub composer_mode: ComposerMode,
    pub focus: Focus,
    pub overlays: OverlayState,
    pub toast: Option<Toast>,
    pub terminal: TerminalMetrics,
}
```

Use bounded collections. A long stream must not cause unbounded memory growth. Recommended initial caps:

- transcript items: 2,000 per selected run;
- tool-output characters per call: 100,000, with truncation marker;
- reasoning characters: 80,000 per run, with collapse state;
- evidence cards: 1,000;
- knowledge cards: 256;
- catalog results: server-defined bounded list.

Store the last seen event sequence per run. On reconnect, call `resume_run` with that cursor. If the server reports a gap or replay failure, reset the projection from a fresh run/session snapshot rather than silently merging uncertain state.

### 6.2 State update rules

Create `AppState::apply_event(envelope: RigaEventEnvelope) -> ApplyOutcome`.

It must:

1. reject or log protocol-version incompatibility;
2. ignore duplicate sequence numbers for the same run;
3. detect gaps and mark the run as `NeedsReplay`;
4. update the selected run only when the event belongs to it;
5. preserve events for non-selected runs in compact run summaries;
6. treat `TextDelta`, `ReasoningDelta`, and `ToolOutputDelta` as streaming deltas;
7. treat plan/todo/graph/task/evidence/knowledge events as authoritative snapshots or records;
8. clear pending approval only when matching `ApprovalResolved` or terminal run state arrives;
9. never infer `Completed` from a missing event;
10. keep `RunCompleted` and `RunFailed` terminal and visible.

### 6.3 Transcript projection

The transcript projection must distinguish:

- user message;
- assistant text;
- assistant reasoning;
- tool call started;
- tool output streaming;
- tool result;
- subagent task result summary;
- system/error notice;
- approval interruption.

Do not flatten tool output into assistant text. Do not display reasoning as if it were the final answer. The user must be able to collapse reasoning and tool details.

---

## 7. Transport implementation

### 7.1 `crates/riga-cli/src/transport.rs`

Implement `RigaTransportClient` around `tokio_tungstenite`.

Responsibilities:

- parse and validate `ws://` and `wss://` URLs;
- connect with bounded handshake timeout;
- send `hello` immediately after WebSocket open;
- wait for `ready` before accepting user commands;
- decode `ServerMessage` and forward it to the app channel;
- serialize `UserCommand` into protocol messages;
- maintain per-run sequence cursors;
- reconnect with bounded exponential backoff;
- issue `resume_run` after reconnect for the interrupted run;
- avoid reconnect loops after explicit user quit;
- distinguish protocol error, server error, network error, and user cancellation;
- ensure one outstanding `list_active_runs` request can be resolved deterministically;
- surface `run_cancelled`, `approval_recorded`, and terminal events.

Do not block the UI task while waiting for a provider response. Do not render from the transport task.

### 7.2 Request correlation

The current protocol has event/run correlation but limited request IDs. Do not add ad hoc IDs only inside the TUI. If a request needs correlation, first extend the shared protocol with an optional request ID and update React/server conformance tests.

For the first implementation, single-flight commands are acceptable for:

- provider configuration;
- active-run listing;
- session creation/listing if those are added to WebSocket protocol.

If sessions/catalog/settings are currently available only through HTTP or Tauri IPC, add a protocol-backed endpoint/message before implementing the TUI feature. The TUI must not call browser-only HTTP routes with undocumented shapes.

### 7.3 Protocol capability gaps

The current browser protocol contract includes sessions, catalog, provider setup, MCP registry, attachments, and local model operations, while the shown WebSocket enum primarily covers run lifecycle and approvals. Resolve this explicitly:

- Extract all currently supported transport operations into shared protocol messages; or
- add a documented CLI adapter endpoint that is still part of RIGA protocol; or
- mark the feature unsupported in the TUI until the protocol exists.

Never silently implement a TUI-only JSON shape.

Create a protocol coverage matrix in `crates/riga-cli/tests/protocol_parity.rs` with columns:

```text
assistant-ui operation | wire message/endpoint | CLI command | implemented | test fixture
```

The build agent must not mark an operation implemented only because a local helper exists.

---

## 8. Ratatui layout

### 8.1 `crates/riga-cli/src/ui.rs`

Rendering must be pure with respect to `AppState` and terminal dimensions. A render function must not send commands, mutate server state, or read files.

Recommended responsive regions:

```text
┌─────────────────────────────────────────────────────────────────┐
│ top bar: RIGA · connection · session · provider · key hints     │
├───────────────────────────────┬─────────────────────────────────┤
│ transcript / tool timeline     │ optional RunDeck side panel    │
│                               │ or overlay                      │
│                               │                                 │
├───────────────────────────────┴─────────────────────────────────┤
│ approval card / error / toast                                  │
├─────────────────────────────────────────────────────────────────┤
│ composer: multiline draft                                      │
│ footer: model · reasoning · RunDeck summary · key hints        │
└─────────────────────────────────────────────────────────────────┘
```

At narrow widths:

- hide or overlay the RunDeck rather than squeezing transcript below readability;
- use a modal overlay for settings/history/catalog;
- keep the composer and approval actions reachable;
- truncate labels but never truncate the actual prompt input;
- show a visible `More`/`Tab` navigation hint.

At short heights:

- prioritize composer, approval, and latest transcript output;
- collapse older tool output and reasoning;
- keep a status line for connection/run state;
- do not make the terminal scroll unexpectedly on every delta.

### 8.2 `crates/riga-cli/src/theme.rs`

Define semantic styles rather than hard-coded styles throughout render functions:

- `background`
- `surface`
- `surface_alt`
- `border`
- `text`
- `muted`
- `accent`
- `success`
- `warning`
- `danger`
- `reasoning`
- `tool`
- `user`
- `assistant`
- `blocked`

Provide a no-color fallback that keeps headings, separators, and focus state legible using text and Unicode/ASCII markers. Respect `--no-color` and terminal color capability detection.

### 8.3 `crates/riga-cli/src/widgets/`

Prefer separate small render modules:

```text
widgets/topbar.rs
widgets/transcript.rs
widgets/tool_card.rs
widgets/reasoning.rs
widgets/composer.rs
widgets/approval.rs
widgets/rundeck.rs
widgets/plan.rs
widgets/todos.rs
widgets/subagents.rs
widgets/graph.rs
widgets/evidence.rs
widgets/knowledge.rs
widgets/session_picker.rs
widgets/settings.rs
widgets/catalog.rs
widgets/active_runs.rs
widgets/help.rs
widgets/toast.rs
```

Each widget should receive immutable state and a `Rect`, and should expose a focused render function. Do not let widgets call the transport.

### 8.4 Transcript and scrollback

`widgets/transcript.rs` must support:

- viewport scrolling with PageUp/PageDown, Up/Down, Home/End;
- follow-output mode while the user is at the bottom;
- automatic follow only when the user has not manually moved away from bottom;
- a `N new events` indicator when follow is disabled;
- a key to jump to bottom;
- preserving scroll position while new streamed deltas arrive;
- separate collapsed/expanded state for reasoning and tool cards.

This mirrors the assistant-ui requirement that streaming should follow only when the user is already at the bottom.

### 8.5 Composer

`widgets/composer.rs` and `input.rs` must support:

- multiline editing;
- Enter sends, Shift+Enter inserts newline;
- up/down history when the cursor is at the relevant boundary;
- draft reset after send;
- draft height bounded by terminal height;
- prompt cancellation with Escape where safe;
- insert menu activation by `/` and `@` only if the catalog is available;
- explicit disabled state while a send command is pending;
- visible stop action while a run is active.

Use a tested text-buffer abstraction. Do not implement cursor movement by slicing UTF-8 strings at arbitrary byte offsets.

### 8.6 Thinking indicator

While the selected run is active and no newer text is visible, show a compact status such as:

```text
RIGA is thinking · 00:12
```

Use task/tool state to enrich it when available:

```text
running build · shell
waiting for approval · write
```

The indicator is presentation only. It must not alter event semantics or manufacture progress.

---

## 9. Feature-parity panels

### 9.1 Sessions/history

Files:

- `src/model.rs`
- `src/widgets/session_picker.rs`
- `src/input.rs`
- `src/app.rs`

Required behavior:

- list sessions with id, title, workspace, and updated metadata;
- select a session without losing the current draft unless explicitly confirmed;
- create a new session with title/workspace validation;
- switch selected run scope when the protocol exposes historical events;
- show an empty state when no session exists;
- never assume a session is active merely because it is listed.

Keyboard proposal:

- `Ctrl+H` open history;
- Up/Down select;
- Enter select;
- `n` create new session;
- Esc close.

The final key map may vary but must be documented in `widgets/help.rs`.

### 9.2 Provider settings

Files:

- `src/widgets/settings.rs`
- `src/model.rs`
- `src/app.rs`
- `src/transport.rs`
- `src/persistence.rs` only for non-secret local preferences

Provider fields must match the React contract:

- endpoint;
- API key input with masked display;
- model;
- reasoning effort: low/medium/high;
- provider kind: remote/local;
- provider API: chat/responses;
- optional subagent model.

Rules:

- API keys must be sent only through the protocol's secure configuration path.
- Never persist API keys in CLI preferences, logs, panic output, command history, or task results.
- Never echo the key into the terminal.
- `Save settings` must be an explicit action because the server persists configuration.
- Local model selection must use the same provider configuration semantics as React.
- Show server errors inline and retain user-entered non-secret fields.

Keyboard proposal:

- `Ctrl+,` open settings;
- Tab/Shift+Tab move fields;
- Enter submit focused action;
- Esc close without saving;
- `Ctrl+S` save settings.

### 9.3 Catalog/command palette

Files:

- `src/widgets/catalog.rs`
- `src/input.rs`
- `src/model.rs`

Display:

- tools;
- skills;
- agents;
- files;
- MCP connectors when returned by catalog.

Use server-provided `Catalog`, `CatalogItem`, `SkillSummary`, `AgentSummary`, and `FileCandidate` semantics. Do not hard-code a second tool list except for local UI help.

The palette must:

- filter incrementally;
- show kind and description;
- insert the server-defined `insert_text` into the draft;
- close after insertion;
- preserve the draft cursor;
- not execute a tool directly from selection.

### 9.4 Approvals

Files:

- `src/widgets/approval.rs`
- `src/app.rs`
- `src/model.rs`

Render an approval card with:

- tool name;
- task id;
- bounded action summary;
- current state: pending/resolved/expired where available;
- deny/allow-once/always controls.

Use the RIGA `ApprovalRequested` and `ApprovalResolved` events. Do not infer risk in the TUI and do not bypass the server policy.

Rules:

- approval actions remain reachable when transcript scrolls;
- `y` allow once, `a` always, `n` deny, Esc leaves pending;
- require explicit confirmation for destructive or persistent actions only if the server protocol says confirmation is required;
- disable duplicate submission while a decision is in flight;
- show an error if the approval id is stale.

### 9.5 RunDeck

Files:

- `src/widgets/rundeck.rs`
- `src/widgets/plan.rs`
- `src/widgets/todos.rs`
- `src/widgets/subagents.rs`
- `src/widgets/graph.rs`
- `src/widgets/evidence.rs`
- `src/widgets/knowledge.rs`
- `src/widgets/active_runs.rs`
- `src/model.rs`

RunDeck must have three lenses matching `docs/Features/RUNDECK.md`:

1. Execution
2. Evidence
3. Knowledge

Collapsed mode:

- one-line summary in the composer/footer area;
- show `done/total`, thinking/running state, ready/blocked counts;
- key to expand.

Expanded mode:

- show above the composer or as a side panel depending on terminal dimensions;
- show current run scope;
- show stop controls for active runs;
- show lens tabs and focused task information.

Execution lens:

- plan card;
- todo card;
- subagent queue;
- graph view when `GraphUpdated` exists;
- dependency fallback list for accessibility and small terminals.

Evidence lens:

- claim;
- source reference;
- confidence;
- task id;
- empty state;
- evidence links.

Knowledge lens:

- fact;
- confidence;
- source run id;
- linked evidence where available;
- empty state.

### 9.6 Execution graph

`widgets/graph.rs` must not require an interactive canvas. Use a deterministic terminal representation:

```text
[✓ explore] ─────▶ [● build] ─────▶ [○ review]
                         │
                         └──────▶ [! test blocked]
```

Requirements:

- topological/dependency-depth ordering;
- stable node ordering across renders;
- state markers for pending/running/waiting/blocked/completed/failed/cancelled;
- selected-node focus;
- direct dependency and dependent navigation;
- horizontal scrolling when needed;
- plain dependency list fallback;
- no graph mutation from the TUI.

Reuse `riga_kernel::task::Graph` and task states. Do not implement a second graph validator.

### 9.7 Subagents

`widgets/subagents.rs` must render:

- profile/agent;
- description;
- model;
- task state;
- elapsed time from `TaskStatus.elapsed_ms`;
- dependency blockers;
- bounded result summary;
- nested tool count and expandable details if events are available.

The TUI must not allow read-only profiles to dispatch or write around server enforcement. Any future re-dispatch/retry button must send a protocol command that the server validates.

### 9.8 Evidence and knowledge

Use the canonical kernel structs. Do not duplicate confidence calculations. If the current protocol has only event creation and no query endpoint, render the events retained for the selected run and document historical limitations.

Bound display text but preserve source ids and references. The user must be able to copy a source reference through a command or terminal selection mode if implemented.

### 9.9 Local models

The React transport exposes:

- overview/catalog;
- download progress;
- download cancellation;
- load;
- unload.

Implement these only through a shared RIGA protocol operation. If WebSocket does not yet expose them, the first TUI phase must add the operation to the protocol and all adapters or explicitly mark local-model management as a later phase.

The UI must show:

- model name;
- size/quant/context;
- installed/loaded/ready states;
- download percentage and bytes;
- cancel control;
- load/unload errors;
- accelerator label.

Do not download a model from the TUI through an ad hoc URL.

---

## 10. Input and command map

Create `crates/riga-cli/src/input.rs` with a central, testable mapping. Avoid scattering raw `KeyCode` checks through widgets.

Recommended global keys:

| Key | Action |
|---|---|
| `Enter` | Send prompt or activate focused action |
| `Shift+Enter` | Newline in composer |
| `Esc` | Close overlay/cancel local editing; never silently cancel a run |
| `Ctrl+C` | First press cancel active input/run confirmation; second press quit, or use explicit quit policy |
| `Ctrl+Q` | Quit after terminal cleanup |
| `Ctrl+H` | Session history |
| `Ctrl+,` | Settings |
| `Ctrl+R` | RunDeck |
| `Ctrl+L` | Clear/refresh visible transcript only, never delete server journal |
| `Tab` | Cycle focus/overlay fields |
| `Shift+Tab` | Reverse focus |
| `PageUp/PageDown` | Transcript scroll |
| `Home/End` | Transcript top/bottom |
| `g` | Jump graph lens or graph focus mode |
| `e` | Evidence lens |
| `k` | Knowledge lens |
| `?` | Help overlay |
| `/` | Tool/skill/catalog palette |
| `@` | Agent/file palette |
| `y` | Approve once when approval focused |
| `a` | Always/session approve when approval focused |
| `n` | Deny when approval focused |

Document conflicts and terminal-specific key limitations. Every key action must have a test where practical.

---

## 11. Persistence rules

`crates/riga-cli/src/persistence.rs` may persist only presentation preferences, such as:

- last server URL;
- selected theme/no-color preference;
- last selected session id;
- RunDeck collapsed/expanded preference;
- keymap preference if supported.

It must not persist:

- API keys;
- provider secrets;
- full prompts unless the RIGA server/session protocol explicitly persists them;
- tool approval grants;
- run/task truth;
- agent results that should come from the server journal.

Use a clearly scoped RIGA data directory consistent with `riga-shell::default_data_dir` if that crate is made a dependency. Do not silently create a second incompatible data root.

---

## 12. Error handling and recovery

Create `crates/riga-cli/src/error.rs` with typed categories:

- invalid CLI arguments;
- terminal initialization/restoration;
- URL/configuration;
- WebSocket connection;
- protocol version mismatch;
- server error;
- replay gap;
- approval stale/expired;
- unsupported protocol operation;
- serialization/deserialization;
- local persistence.

User-facing errors must include:

1. a short title;
2. a safe message;
3. whether retrying is useful;
4. a key to dismiss or reconnect.

Never show API keys, authorization headers, raw provider request bodies, or full panic backtraces in the TUI.

On disconnect:

- preserve the current transcript and draft;
- show `disconnected` in the top bar;
- retry with bounded backoff;
- on reconnect, handshake again;
- replay the selected active run from `last_sequence`;
- reconcile `active_runs`;
- show a gap/recovery notice if replay is incomplete.

On `local_run_in_progress`:

- do not start another run automatically;
- list active runs;
- allow selecting and stopping the stale/current run;
- retain the exact server error in a bounded error card.

---

## 13. Tests and fixtures

### 13.1 New CLI unit tests

Create:

```text
crates/riga-cli/src/input.rs       unit tests for key mapping and text editing
crates/riga-cli/src/model.rs       event projection, dedup, gap, terminal tests
crates/riga-cli/src/format.rs      truncation, markdown/plain text, UTF-8 tests
crates/riga-cli/src/transport.rs   message serialization and reconnect state tests
crates/riga-cli/src/theme.rs       no-color and narrow-terminal tests
```

### 13.2 Integration tests

Create `crates/riga-cli/tests/`:

```text
protocol_parity.rs
fake_server.rs
reconnect_replay.rs
approval_flow.rs
run_projection.rs
catalog_flow.rs
session_flow.rs
terminal_layout.rs
```



Test scenarios:

1. handshake succeeds and protocol version is displayed;
2. unsupported protocol version fails safely;
3. ordered text deltas produce one assistant message;
4. reasoning deltas remain separate/collapsible;
5. tool call/output/result cards preserve call id;
6. approval request blocks the run until allow/deny;
7. always approval sends `option: "always"`;
8. run completion is terminal and visible;
9. run failure is terminal and visible;
10. duplicate event sequence is ignored;
11. sequence gap triggers replay;
12. reconnect resumes after the last sequence;
13. active runs list displays and cancel sends the correct run id;
14. plan/todo/graph events update the correct run;
15. evidence and knowledge lenses render empty and non-empty states;
16. graph rendering is deterministic;
17. transcript stops following when user scrolls upward;
18. transcript resumes following after jump-to-bottom;
19. composer sends Enter and inserts Shift+Enter newline;
20. draft height/input remains valid for Unicode;
21. settings never logs or persists API keys;
22. catalog insertion updates the draft without executing a tool;
23. local-model progress/cancel/load state is rendered if protocol support exists;
24. terminal cleanup runs after transport error and user quit.

### 13.3 Cross-client conformance

Add shared JSON fixtures under a protocol-owned test fixture directory, for example:

```text
crates/riga-kernel/tests/fixtures/protocol/
```

Include:

- every `RigaEvent` variant;
- every client/server message;
- optional-field compatibility cases;
- error cases;
- task/graph/evidence/knowledge examples.

Use those fixtures from Rust tests and TypeScript conformance tests. The TUI must decode the same bytes that assistant-ui accepts.

### 13.4 Snapshot/render tests

Use deterministic `Terminal::with_backend(TestBackend)` snapshots for:

- idle screen;
- active run;
- approval card;
- RunDeck execution lens;
- evidence lens;
- knowledge lens;
- settings overlay;
- narrow terminal;
- no-color mode;
- disconnected/reconnecting state.

Snapshots must assert structure and text, not terminal escape sequences.

---

## 14. Implementation phases

### Phase TUI-0 — IPC transport seam and headless smoke

Files:

- `crates/riga-cli/Cargo.toml`
- `crates/riga-cli/src/main.rs`
- `crates/riga-cli/src/transport/mod.rs` (new)
- `crates/riga-cli/src/transport/ipc.rs` (new)
- `crates/riga-cli/src/model.rs` (new)
- `crates/riga-cli/tests/ipc_transport.rs` (new)
- `docs/BuildPlan-TUI/phase-0-report.md`

Exit criteria:

- `riga-cli --plain` uses `IpcService` by default;
- health, session listing, start/subscribe/cancel, active-run listing, and approval paths have a CLI-owned transport seam;
- deterministic in-process IPC tests pass without an external server or provider;
- typed `RigaEventEnvelope` values are preserved;
- no WebSocket extraction is required;
- no API key is printed or persisted by the CLI;
- the scaffold is replaced by a documented executable entrypoint.

### Phase TUI-1 — Headless projection and optional WebSocket adapter

Files:

- `crates/riga-cli/src/transport/websocket.rs` (new, optional)
- `crates/riga-cli/src/app.rs`
- `crates/riga-cli/src/model.rs`
- `crates/riga-cli/src/error.rs`
- `crates/riga-cli/tests/event_projection.rs`
- `crates/riga-cli/tests/websocket_transport.rs` (only if adapter is enabled)

Exit criteria:

- headless output handles ordered text/reasoning/tool/approval/task/evidence/knowledge/terminal events;
- sequence deduplication and resume semantics are tested;
- IPC remains the default when no transport flag is supplied;
- optional WebSocket mode is explicitly selected and does not change IPC behavior;
- WebSocket message extraction is deferred unless duplicated types create a measured problem.

### Phase TUI-2 — Terminal lifecycle and idle screen

Files:

- `src/terminal.rs`
- `src/ui.rs`
- `src/theme.rs`
- `src/input.rs`
- `src/widgets/topbar.rs`
- `src/widgets/composer.rs`
- `src/widgets/help.rs`

Exit criteria:

- terminal is always restored;
- idle screen is usable at 80×24 and 120×40;
- multiline composer and send flow work against fake server;
- no rendering code performs I/O.

### Phase TUI-3 — Streaming transcript and tools

Files:

- `src/model.rs`
- `src/format.rs`
- `src/widgets/transcript.rs`
- `src/widgets/tool_card.rs`
- `src/widgets/reasoning.rs`
- `src/widgets/approval.rs`

Exit criteria:

- text/reasoning/tool streams render in order;
- scroll-follow behavior is tested;
- approvals block visibly and resolve through protocol;
- failures and terminal events are clear.

### Phase TUI-4 — Sessions, settings, catalog

Files:

- `src/widgets/session_picker.rs`
- `src/widgets/settings.rs`
- `src/widgets/catalog.rs`
- `src/persistence.rs`
- protocol/server adapter files required for missing operations

Exit criteria:

- no secrets are persisted or logged;
- session and provider operations use shared protocol messages;
- catalog insertion works;
- unsupported operations are explicit.

### Phase TUI-5 — RunDeck parity

Files:

- `src/widgets/rundeck.rs`
- `src/widgets/plan.rs`
- `src/widgets/todos.rs`
- `src/widgets/subagents.rs`
- `src/widgets/graph.rs`
- `src/widgets/evidence.rs`
- `src/widgets/knowledge.rs`
- `src/widgets/active_runs.rs`

Exit criteria:

- all three lenses render;
- active run recovery and stop work;
- graph fallback is usable without a mouse;
- evidence/knowledge use canonical event data;
- narrow terminals remain usable.

### Phase TUI-6 — Local models and attachment parity

Files depend on protocol extraction:

- `src/widgets/local_models.rs` (new)
- `src/widgets/attachments.rs` (new if supported)
- `src/model.rs`
- `src/transport.rs`
- shared server/kernel protocol files

Exit criteria:

- local model operations are protocol-backed;
- progress and cancellation are deterministic;
- attachment behavior is either fully supported through RIGA protocol or clearly marked unavailable;
- no TUI-only endpoint exists.

### Phase TUI-7 — Hardening and release

Files:

- `crates/riga-cli/tests/*`
- workspace CI configuration if required;
- `README.md` CLI section;
- `docs/Features/` only for behavior that becomes a shared product contract;
- `docs/BuildPlan-TUI-phase-*.md` report.

Exit criteria:

- all workspace tests pass;
- terminal snapshots pass;
- protocol conformance passes for Rust and TypeScript;
- `cargo clippy` has no new warnings;
- clean terminal restoration is verified after each error class;
- a phase report lists exact commands and known limitations.

---

## 15. Required validation commands

Run from repository root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p riga-cli
npm run check
npm test -- --run
npm run build --prefix demo
```

If TUI snapshots use a feature flag, run both:

```bash
cargo test -p riga-cli
cargo test -p riga-cli --features tui-snapshots
```

The coding agent must also run a manual smoke test with a fake/local RIGA server and record:

- terminal size;
- server URL;
- protocol version;
- tested flow;
- whether the terminal was restored after quit and failure.

Do not put API keys in command history, logs, screenshots, fixtures, or reports.

---

## 16. Definition of done

The TUI work is complete only when:

- `riga-cli` is no longer a scaffold;
- it is a protocol client, not a second agent runtime;
- it uses the same `RigaEventEnvelope`, task states, graph, evidence, knowledge, and approval semantics as assistant-ui;
- React and Ratatui consume conformance fixtures with equivalent event meaning;
- all supported assistant-ui operations have a CLI mapping or an explicit unsupported status;

- no external reference source code was copied or adapted;
- terminal restoration is reliable;
- streaming, scrollback, approvals, reconnection, and active-run cancellation are tested;
- RunDeck execution/evidence/knowledge lenses are available;
- settings/catalog/session flows do not leak secrets;
- small terminal dimensions have tested fallbacks;
- the full validation gate passes;
- the final phase report identifies files changed, protocol changes, tests, limitations, and follow-up work.

## 17. Coding-agent execution prompt

Use this prompt when assigning the implementation:

```text
Implement docs/BuildPlan-TUI.md from Phase TUI-0 onward. IPC is the default transport; WebSocket is optional. Read the complete handoff,
crates/riga-kernel/src/events.rs, crates/riga-kernel/src/task.rs,
crates/riga-kernel/src/policy.rs, packages/assistant-ui/src/protocol/index.ts,
and docs/Features/RUNDECK.md, SUBAGENT.md, EVIDENCE.md, and KNOWLEDGE.md first.



Implement one phase at a time. Start with the public `riga_server::ipc::IpcService` adapter. Keep riga-cli a thin
client and keep all agent/provider/tool/policy logic in the existing RIGA runtime.
Do not extract WebSocket messages to `riga-kernel` unless a later optional adapter
requires it and the need is demonstrated.
Do not add a second agent loop. Add focused Rust tests, fake-server protocol tests,
React conformance fixtures, and Ratatui TestBackend snapshots as each phase requires.

At the end of each phase return:
- Status: PASS or BLOCKED
- Phase and exit criteria completed
- Files changed
- Protocol changes
- Exact test commands and results
- Unsupported operations or known risks
- Next phase allowed: YES or NO
```

## 18. Reference note

