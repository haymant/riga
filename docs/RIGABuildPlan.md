# RIGA Build Plan

> **Purpose:** Detailed implementation plan for the RIGA coding-agent platform.
>
> **Inputs:** `PLAN-RIG-CODING-APP.md`, the attached rig-agent proposal, assistant-ui documentation, the Fina Builder architecture, and Create.xyz integration guidance.
>
> **Phase:** Design and planning only. This document is the handoff contract for the coding agent that will implement, test, package, and release RIGA.

## 1. Decisions before coding

### 1.1 Product name

The product is **RIGA**. The old name `rig-coding-app` is retired.

Use these names:

- desktop app: `riga`;
- Rust kernel crate: `riga-kernel`;
- React package: `@riga/assistant-ui`;
- optional browser transport: `@riga/transport-http`;
- optional Tauri transport: `@riga/transport-tauri`;
- server adapter: `riga-server`;
- command-line adapter: `riga-cli`.

Keep `rig-agent` as an upstream dependency name where it is actually the dependency being used.

### 1.2 Distribution decision

There are three different distribution targets:

| Artifact | Distribution | Consumer |
|---|---|---|
| `riga-kernel` | crates.io and GitHub source/release | Rust/Tauri applications |
| `@riga/assistant-ui` | npm and GitHub Packages | React applications |
| `riga-server` | container/binary/GitHub Release | Web/Create.xyz clients |
| `riga` desktop | Tauri GitHub Release assets | End users |
| OpenAPI contract | GitHub Release/docs site | Create.xyz import and API clients |

Create.xyz should not be used as a Rust crate registry. Create.xyz’s documented integration path is to bring an own REST API through Swagger/OpenAPI, export the generated code, and publish the generated application. [6] The kernel is therefore published through Rust/package infrastructure; Create.xyz consumes a restricted server adapter.

### 1.3 Architecture decision

The core rule is copied from Fina Builder:

> Transport adapters deserialize, call the kernel, and serialize. They do not contain business logic. [5]

This rule must be enforced through crate dependencies, module boundaries, tests, and code review.

## 2. Proposed repository layout

Start with a monorepo so the wire contract, Rust kernel, React UI, Tauri app, server adapter, fixtures, and release automation evolve together.

```text
riga/
├── Cargo.toml                         # workspace
├── package.json                       # frontend workspace scripts
├── pnpm-workspace.yaml                # optional; use npm workspaces if simpler
├── rust-toolchain.toml
├── README.md
├── PLAN-RIG-CODING-APP.md
├── RIGA-BUILD-PLAN.md
├── LICENSE
├── crates/
│   ├── riga-kernel/
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── api.rs
│   │   │   ├── config.rs
│   │   │   ├── error.rs
│   │   │   ├── events.rs
│   │   │   ├── kernel.rs
│   │   │   ├── sessions.rs
│   │   │   ├── runs.rs
│   │   │   ├── tools.rs
│   │   │   ├── approvals.rs
│   │   │   ├── policy.rs
│   │   │   ├── persistence.rs
│   │   │   ├── artifacts.rs
│   │   │   └── testing.rs
│   │   └── tests/
│   ├── riga-server/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── main.rs
│   │       ├── routes.rs
│   │       ├── auth.rs
│   │       └── openapi.rs
│   └── riga-cli/
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           └── commands.rs
├── packages/
│   ├── assistant-ui/
│   │   ├── package.json
│   │   ├── src/
│   │   │   ├── index.ts
│   │   │   ├── runtime/
│   │   │   ├── components/
│   │   │   ├── tools/
│   │   │   ├── protocol/
│   │   │   └── styles/
│   │   └── tests/
│   ├── transport-http/
│   └── transport-tauri/
├── apps/
│   └── riga/
│       ├── src/                         # React composition root
│       ├── src-tauri/
│       │   ├── Cargo.toml
│       │   ├── src/main.rs
│       │   ├── src/lib.rs
│       │   ├── src/commands.rs
│       │   └── capabilities/
│       └── src-tauri/tauri.conf.json
├── openapi/
│   └── riga-server.yaml
├── fixtures/
│   ├── events/
│   ├── sessions/
│   └── tools/
├── scripts/
│   ├── set-version.mjs
│   ├── check-public-api.mjs
│   ├── generate-openapi.mjs
│   └── verify-package-boundaries.py
└── .github/workflows/
    ├── ci.yml
    ├── release-kernel.yml
    ├── release-ui.yml
    ├── release-server.yml
    └── release-desktop.yml
```

If the first implementation is kept in the existing Fina Builder repository, preserve this logical separation even if the initial directory layout is flatter. The current Fina Builder architecture already separates a transport-free kernel from Tauri, HTTP, CLI, and MCP adapters and uses parity tests and dependency hygiene to prevent drift. [5]

## 2.1 Rig is the agent engine; RIGA is the product runtime

The implementation must not reinvent capabilities already present in Rig. Rig is an LLM application framework, not merely an HTTP client. Its current workspace separates provider-neutral contracts in `rig-core` from the classic agent runtime in `rig-agent`; the repository identifies completion models, normalized messages, contextual tools, memory/vector-store contracts, provider mappings, agent builders, streaming traits, hooks, extraction, and serializable `AgentRun` state as existing Rig capabilities. [11]

### Responsibility matrix

| Concern | Reuse from Rig | Implement in RIGA |
|---|---|---|
| LLM provider abstraction | `ProviderClient`, `CompletionClient`, `CompletionModel`, provider clients | Provider selection/config UI and product-specific defaults |
| OpenAI-compatible providers | Rig OpenAI client builder with `.base_url(...)` and model IDs | OpenCode Go endpoint/model config, session header and user-agent injection when necessary |
| Prompting | `Prompt`, `Chat`, completion request builders | RIGA request DTOs and application prompt templates |
| Agent loop | `AgentBuilder`, `Agent`, `AgentRunner`, `AgentRun`, `max_turns` | Run registry, durable run identity, cancellation command, journal, recovery UI |
| Tool calling | `Tool` trait, `ToolDefinition`, argument deserialization, tool results, tool-call loop | Concrete coding tools and security/policy wrapper |
| Tool discovery | static `.tool(...)`, dynamic tools/vector retrieval | Workspace/project tool catalog, permission-filtered active tools |
| Tool-call recovery | invalid-call handling and `AgentHook` flow actions | Product error display, audit records, approval UX, hard security enforcement |
| Text streaming | `StreamingPrompt`, `StreamingChat`, `StreamingCompletion`, `MultiTurnStreamItem` | Convert Rig stream items into `RigaEventEnvelope`, backpressure, transport fan-out |
| Tool-call streaming | streamed text/tool deltas and final usage | Buffering policy, user-visible tool cards, approval pause/resume |
| Hooks | `AgentHook`, `StepEvent`, `Flow`, hook stack | Audit hook, metrics hook, approval hook, redaction hook, policy integration |
| Conversation memory | `ConversationMemory`, in-memory backend, history shaping policies | Durable session/thread store, migrations, retention, encryption, user-visible branches |
| Structured results | `TypedPrompt`, extractors, schemas, `serde` integration | RIGA command/artifact schemas and domain validation |
| Embeddings/RAG | embedding models, vector stores, dynamic context/tools | Indexing workspace code, consent, tenancy, invalidation, UI |
| Model usage | `Usage`, `GetTokenUsage`, raw provider response access | Cost meter, quotas, redaction, persistence, analytics |
| Deterministic testing | `MockCompletionModel`, `MockTurn`, `MockEmbeddingModel`, request inspection, cassette/replay support | Kernel fixtures, fake tools, transport parity, component/E2E tests |
| Provider extension | Rig `CompletionClient`/`CompletionModel` extension points | Only a thin compatibility adapter when an endpoint cannot use a built-in provider |

Rig’s agent already constructs a request from the preamble, context, history, and tool definitions, sends it to the model, executes tool calls, feeds results back, and repeats until a final response or turn budget is reached. [12] RIGA must invoke and observe this loop rather than create a second `RigaAgentLoop`.

### Crates and modules to use

Prefer the following dependency shape, subject to the pinned Rig release:

```toml
[dependencies]
rig-core = "<pinned-version>"
rig-agent = "<pinned-version>"
# Or the feature-gated `rig` facade when companion crates are needed:
# rig = { version = "<pinned-version>", features = ["..." ] }
```

Use:

- `rig-core` for provider-neutral messages, completion models, tools, memory/vector-store contracts, usage, and shared errors;
- `rig-agent` for `AgentBuilder`, agent prompting, streaming, hooks, live tool registration, extraction, and `AgentRun`;
- `rig` facade only when its feature-gated integrations reduce integration complexity;
- `rig-memory` only when RIGA wants a reusable history policy such as sliding/token windows or compaction;
- Rig provider companion crates only for the required backend;
- `rig::test_utils` as a dev dependency for deterministic mock models and scripted turns;
- `rig-cassette`/cassette support for provider-effect recording and replay where compatible with the pinned release.

Do not copy Rig internals into `riga-kernel`. If an API is missing, isolate a small adapter or contribute upstream.

### Concrete composition

```text
riga-kernel
├── rig_compat/
│   ├── provider.rs       # Rig client/model construction
│   ├── agent.rs          # AgentBuilder/Runner construction
│   ├── streaming.rs      # MultiTurnStreamItem conversion
│   ├── hooks.rs          # AgentHook registration
│   ├── memory.rs         # ConversationMemory adapter
│   └── testing.rs        # Rig mocks/cassettes behind cfg(test)
├── coding_tools/
│   ├── read_file.rs      # implements Rig Tool
│   ├── write_file.rs     # implements Rig Tool + RIGA policy
│   ├── apply_patch.rs    # implements Rig Tool + approval
│   └── run_command.rs    # implements Rig Tool + sandbox
├── policy/               # RIGA authoritative policy, not model-facing advice
├── sessions/             # RIGA durable sessions and UI threads
├── events/               # RIGA journal and wire envelopes
└── transports/           # Tauri/HTTP/CLI adapters
```

### What must not be implemented again

The coding agent must reject or redesign any implementation that introduces:

- a second model-provider trait;
- a second normalized message/content enum;
- a second agent tool-call loop;
- a second generic streaming trait;
- a second generic conversation-memory trait;
- a second generic token-usage type;
- a second generic provider error taxonomy;
- hand-written OpenAI-compatible JSON when Rig’s OpenAI provider already supports the endpoint;
- RIGA-only fake model abstractions when Rig’s `test_utils` can provide the behavior.

A RIGA wrapper is allowed when it adds application semantics—IDs, authorization, persistence, event envelopes, or UI mapping—but it must delegate to Rig and preserve the underlying result/error meaning.

### Hooks and security boundary

Rig hooks are the correct extension point for audit, metrics, request shaping, invalid-tool recovery, approval pauses, and streaming UI integration. Rig’s hook API exposes `StepEvent` for model calls, responses, tool calls/results, streamed deltas, and invalid tool calls; `Flow` can continue, terminate, skip, rewrite, retry, repair, or override a request. [14]

RIGA should register hooks for observation and orchestration, but hooks alone are not the security boundary. The authoritative check must occur in the coding tool or downstream executor after resolving the real workspace, capability, user/session identity, approval, and policy. A malicious or faulty model must not bypass security by avoiding a hook.

### Memory boundary

Rig provides the `ConversationMemory` contract and an in-memory implementation, and its memory companion supports bounded windows and compaction policies. [16] RIGA should implement a durable adapter over its session store rather than create another history abstraction. RIGA owns the product-level thread/branch model and can pass a shaped `Vec<Message>` to Rig when it needs exact control. Be careful with Rig’s documented distinction: `with_history` bypasses automatic memory and does not append the new turn, while `chat` does append. [16]

### Testing boundary

Use Rig’s `MockCompletionModel`, `MockTurn`, `MockEmbeddingModel`, request inspection, and cassette/replay facilities first. Rig’s test documentation explicitly supports offline, deterministic agent tests and scripted multi-turn tool calls. [15]

RIGA tests should then add what Rig cannot know about:

- workspace/policy/security behavior;
- durable sessions and event replay;
- transport parity;
- Tauri lifecycle;
- assistant-ui rendering and accessibility;
- approval UX;
- crash/reconnect recovery;
- OpenCode Go live compatibility.

### OpenCode Go implementation consequence

OpenCode Go is not a reason to build a new provider from scratch. Start with Rig’s OpenAI provider and custom base URL. Rig documents that its OpenAI client is reusable across OpenAI-compatible providers through `base_url`. [8] Only add a small RIGA provider compatibility layer if the selected OpenCode Go model requires a protocol family or header behavior that the pinned Rig provider cannot express.

OpenCode Go’s current documentation lists different endpoint families for different models and recommends a stable `x-opencode-session` header plus an identifying user agent. [10] Therefore:

1. Resolve the model catalog/configuration in RIGA.
2. Select the protocol-compatible Rig model/client path.
3. Attach `x-opencode-session` and `riga/<version>` at the HTTP boundary.
4. Convert Rig’s normalized stream/events into RIGA events.
5. Keep the provider key in the RIGA secret/config layer only.

Do not make the RIGA UI or `@riga/assistant-ui` aware of OpenCode-specific request JSON.

### Rig versioning rule

Pin the Rig version and isolate imports in `rig_compat`. Rig’s repository warns that breaking changes are expected while the project evolves. [11] Add a compatibility test that confirms the pinned release still provides the required agent, streaming, hook, memory, and test-utils APIs. Upgrade Rig in a dedicated change with migration notes, fixture updates, and transport-parity verification.

## 3. Rust kernel architecture

### 3.1 Dependency policy

`riga-kernel` must have no Tauri, Actix, Axum, browser, filesystem UI, or HTTP-server dependency. A first version may use:

```toml
[dependencies]
anyhow = "1"
futures-core = "0.3"
futures-util = "0.3"
rig-agent = "<pinned-version>"
rig-core = "<pinned-version>"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["sync", "time"] }
tracing = "0.1"
```

Keep the dependency list smaller if the pinned rig version provides equivalent types. Add a dependency-hygiene test that fails when an adapter-only crate appears in `riga-kernel/Cargo.toml`.

### 3.2 Kernel state

```rust
pub struct RigaKernel {
    config: RigaConfig,
    rig_runtime: RigRuntime,
    sessions: SessionStore,
    runs: RunStore,
    tools: ToolRegistry,
    policies: PolicyRegistry,
    journal: EventJournal,
    model_registry: ModelRegistry,
}
```

The fields are private. Public methods operate on requests and return stable domain types. Do not expose a `rig_agent::Agent` in the public API.

### 3.3 Construction and lifecycle

```rust
impl RigaKernel {
    pub async fn new(config: RigaConfig) -> Result<Self, RigaError> {
        let rig_runtime = rig_compat::build_runtime(&config).await?;
        let tools = ToolRegistry::from_config(&config)?;
        let models = ModelRegistry::from_config(&config)?;
        let journal = EventJournal::open(&config.data_dir).await?;

        Ok(Self {
            config,
            rig_runtime,
            sessions: SessionStore::open(&config.data_dir).await?,
            runs: RunStore::open(&config.data_dir).await?,
            tools,
            policies: PolicyRegistry::from_config(&config),
            journal,
            model_registry: models,
        })
    }
}
```

The precise `RigRuntime` wrapper and builder calls must be adapted to the pinned Rig release. The coding agent must first build a minimal Rig `AgentBuilder`/`AgentRunner` proof of concept and confirm the current API. Do not invent an additional bus abstraction unless the pinned Rig version actually requires one; the default design is to own a Rig agent/runner through `rig_compat` and expose only RIGA lifecycle operations.

### 3.4 Run state machine

Every run moves through a finite state machine:

```text
Created → Queued → Running
Running → WaitingForApproval → Running
Running → Paused → Resuming → Running
Running → Completed
Running → Failed
Running → Cancelled
```

Invalid transitions must return a typed `INVALID_STATE_TRANSITION` error. A cancelled run must not emit a later `RunCompleted` event. The journal sequence is the source of truth if a UI reconnects.

### 3.5 Session persistence

Persist:

- session metadata;
- message tree and branch IDs;
- run metadata;
- serialized agent/run state where supported by the pinned rig version;
- event journal cursor;
- checkpoints;
- artifacts and file hashes;
- approval decisions and audit metadata.

Use an application data directory supplied by the host. Do not assume the current working directory is writable. Store each session under a safe generated identifier, not a user-provided filename.

The first release may use JSON files behind a `SessionStore` trait. Design the trait so SQLite can replace it without changing the kernel API.

```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create(&self, request: CreateSessionRequest) -> Result<Session, RigaError>;
    async fn get(&self, id: &SessionId) -> Result<Option<Session>, RigaError>;
    async fn save(&self, session: &Session) -> Result<(), RigaError>;
    async fn list(&self) -> Result<Vec<SessionSummary>, RigaError>;
}
```

### 3.6 Tool registry and policy

Represent tools as a public descriptor plus a private executor:

```rust
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub capabilities: Vec<Capability>,
    pub approval: ApprovalMode,
}

pub enum ApprovalMode {
    Never,
    OnFirstUse,
    EveryUse,
    AlwaysForDestructive,
}
```

Tool executors must receive a scoped `ToolContext` containing the session, workspace, cancellation token, policy decision, and event sink. They must not receive unrestricted global application state.

Initial tools:

- `read_file`;
- `search_files`;
- `list_directory`;
- `write_file`;
- `apply_patch`;
- `run_command`;
- `git_status`;
- `git_diff`;
- `git_commit`;
- `create_artifact`;
- `read_artifact`;
- MCP-proxied tools.

### 3.7 Errors

Use stable machine-readable codes:

```rust
pub enum RigaErrorCode {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    SessionNotFound,
    RunNotFound,
    InvalidStateTransition,
    ToolNotFound,
    ApprovalRequired,
    PolicyDenied,
    WorkspaceEscape,
    CommandTimeout,
    ModelUnavailable,
    ProviderError,
    PersistenceError,
    Internal,
}
```

Serialize errors as `{ code, message, retryable, details }`. Do not expose raw provider secrets, local paths outside the workspace, or Rust backtraces in the public error message.

## 4. Wire protocol

### 4.1 Commands

The kernel dispatcher should define the command table once:

```rust
pub enum KernelCommand {
    CreateSession(CreateSessionRequest),
    ListSessions(ListSessionsRequest),
    Submit(SubmitRequest),
    GetThread(GetThreadRequest),
    Stream(StreamRequest),
    Cancel(CancelRequest),
    Pause(PauseRequest),
    Resume(ResumeRequest),
    Approve(ApproveRequest),
    Reject(RejectRequest),
    CreateCheckpoint(CreateCheckpointRequest),
    RestoreCheckpoint(RestoreCheckpointRequest),
    ListArtifacts(ListArtifactsRequest),
    ReadArtifact(ReadArtifactRequest),
    GetCapabilities(GetCapabilitiesRequest),
}
```

The exact number of commands can be reduced for the first milestone, but the dispatcher must remain the one source of truth for Tauri, HTTP, CLI, and tests.

### 4.2 Event streaming

The preferred wire shape is a replayable event stream:

```http
GET /v1/sessions/{session_id}/runs/{run_id}/events?after_sequence=42
Accept: text/event-stream
```

Tauri uses a typed `Channel<RigaEventEnvelope>` or equivalent v2 channel. The event payload is identical in both transports. A client can reconnect using `after_sequence` and must deduplicate by `event_id` or sequence.

### 4.3 Protocol compatibility

- Include `protocol_version` in every envelope.
- Add fields compatibly; do not rename fields in place.
- Keep deprecated fields for at least one minor version.
- Reject a newer unsupported protocol with a clear `UNSUPPORTED_PROTOCOL` error.
- Generate TypeScript types from the Rust/OpenAPI contract where practical, then commit the generated output and check for drift in CI.

## 5. React package implementation plan

### 5.1 Package boundary

`@riga/assistant-ui` contains:

- assistant-ui provider/runtime integration;
- RIGA wire types;
- transport interfaces;
- tool renderers;
- composable components;
- CSS variables and accessible defaults;
- test fixtures and a mock runtime.

It must not contain:

- Rust/Tauri imports in the core package;
- provider API keys;
- filesystem or shell execution;
- direct assumptions about a particular backend URL;
- domain computations that belong in the kernel.

### 5.2 Runtime adapters

Implement in this order:

1. `MockRigaRuntime` for tests and Storybook-like development.
2. `RigaAssistantTransport` for structured event snapshots.
3. `RigaHttpRuntime` for REST/SSE.
4. `RigaTauriRuntime` for `invoke` plus Tauri channel.
5. `RigaExternalStoreRuntime` for hosts that already own session state.

The custom-runtime choice follows assistant-ui’s documented decision tree. `LocalRuntime` is suitable for a simple adapter, while `AssistantTransport` is intended for structured state streaming and bidirectional commands. [2]

### 5.3 Component plan

Build components in dependency order.

#### Foundation

- `RigaRuntimeProvider`;
- `RigaErrorBoundary`;
- `RigaThemeProvider`;
- `RigaShell`;
- `RigaResizablePanel`;
- `RigaCommandPalette`;
- `RigaHotkeys`;
- `RigaAccessibilityAnnouncer`.

#### Thread and composer

- `RigaThread` composed from assistant-ui `Thread` or primitives;
- `RigaMessage` and role-specific message renderers;
- `RigaMarkdown`;
- `RigaComposer`;
- `RigaAttachmentList`;
- `RigaMentionPicker`;
- `RigaSlashCommandPicker`;
- `RigaModelPicker`;
- `RigaContextMeter`;
- `RigaFollowupSuggestions`;
- `RigaBranchPicker`;
- `RigaThreadList`.

assistant-ui’s `Thread` already defines the expected composition of viewport, messages, footer, suggestions, and composer, and supports component slot overrides. [3] Prefer slot overrides and primitive composition over forking the whole component.

#### Coding views

- `RigaAgentCard`;
- `RigaAgentStatus`;
- `RigaPlan`;
- `RigaCheckpointList`;
- `RigaToolCallGroup`;
- `RigaApprovalCard`;
- `RigaElicitationForm`;
- `RigaCodeDiff`;
- `RigaCodeRunner`;
- `RigaTerminalOutput`;
- `RigaFileTree`;
- `RigaArtifactCard`;
- `RigaCanvas`;
- `RigaConnectionState`;
- `RigaBackgroundRunCard`.

#### Research and structured output

- `RigaCitation`;
- `RigaDocumentReference`;
- `RigaConfidence`;
- `RigaDataTable`;
- `RigaChart`;
- `RigaDiagram`;
- `RigaImage`;
- `RigaImageGallery`;
- `RigaLinkPreview`.

#### Feedback and recovery

- `RigaEmptyState`;
- `RigaLoadingState`;
- `RigaErrorState`;
- `RigaRetryButton`;
- `RigaFeedbackDialog`;
- `RigaGuardrailNotice`;
- `RigaDraftRestore`.

### 5.4 Tool renderer registration

Use a tool-name registry. Kernel-executed tools are external from the browser’s point of view. assistant-ui documents `externalTool()` for tools executed by another system while the client owns rendering. [4]

```ts
export const rigaToolRenderers = {
  read_file: ReadFileToolCard,
  search_files: SearchFilesToolCard,
  write_file: WriteFileApprovalCard,
  apply_patch: ApplyPatchApprovalCard,
  run_command: RunCommandCard,
  git_diff: GitDiffCard,
  create_artifact: ArtifactCard,
};
```

Every renderer supports `running`, `complete`, `error`, `cancelled`, and `approval_required`. The renderer must not fabricate a result while the tool is still running.

### 5.5 React state rules

- Runtime state is authoritative for messages and run status.
- Zustand, if used, stores layout preferences, UI preferences, and cached view state—not agent business state that could diverge from the kernel.
- Selectors must return stable references to avoid the React `getSnapshot` infinite-loop failure previously seen in Fina Builder.
- Every async view handles `idle`, `loading`, `ready`, `error`, and `empty`.
- Abort/cancel is idempotent.
- Event replay is deduplicated.
- Components are keyboard accessible and announce status changes through an ARIA live region.

## 6. Tauri integration

### 6.1 Thin adapter

`apps/riga/src-tauri` owns:

- application startup;
- managed `Arc<RigaKernel>` state;
- Tauri command registration;
- channel/event forwarding;
- filesystem path discovery;
- OS menus, tray, notifications, updater, and window behavior;
- capability configuration.

Example shape:

```rust
#[tauri::command]
async fn submit(
    state: State<'_, RigaState>,
    request: SubmitRequest,
) -> Result<RunAccepted, RigaErrorWire> {
    state.kernel.submit(request).await.map_err(Into::into)
}
```

No tool execution, path validation formula, model selection branch, or domain default belongs in this function.

### 6.2 Capability policy

Start with the smallest Tauri v2 capability set:

- core app/window operations;
- event/channel emission;
- scoped filesystem access to the configured workspace and app data directory;
- scoped shell execution only if the kernel is the authority and the Tauri shell capability is required;
- updater only in signed production builds.

Prefer kernel-owned process execution over broad frontend shell permissions. If Tauri shell access is needed, keep it scoped and test the capability file in CI.

### 6.3 Mandatory process isolation and Tauri security requirements

#### Important terminology

A Bash invocation is only a child process. It is **not** a sandbox. The Tauri shell plugin is also not a general OS sandbox: it lets a frontend invoke scoped child processes, while the Rust application core and plugins retain system access. [20] [21]

Tauri’s capabilities and permissions protect the WebView-to-Tauri IPC boundary. Tauri explicitly states that code in plugins or the application core has full access to available system resources and is not constrained by the WebView capability model. Therefore, a Rust command that launches `Command::new("bash")` must enforce its own policy and OS isolation. [20]

Tauri’s Isolation Pattern is different again: it inserts a small sandboxed iframe application between the frontend and Tauri Core to intercept, validate, and encrypt IPC messages. It protects the IPC boundary from compromised frontend/dependency code; it does **not** sandbox a Rust child process, filesystem, shell, or network. [23]

RIGA must use the following terms precisely:

- **Child-process execution:** launching a process; no meaningful isolation by itself.
- **Restricted execution:** policy checks, sanitized environment, workspace containment, resource limits, timeout, and process-tree cleanup.
- **OS-sandboxed execution:** OS-enforced filesystem/process/network/resource isolation around the child process.
- **Disposable execution:** OS-sandboxed execution inside a temporary container or VM whose writable state is destroyed after the run except for explicitly exported artifacts.

#### Mandatory architecture

All coding-agent commands must flow through this path:

```text
React / assistant-ui
  → typed Tauri command
  → riga-kernel policy and approval
  → execution supervisor
  → restricted or OS-sandboxed worker
  → child process
```

The frontend must never call `@tauri-apps/plugin-shell` for agent commands. The shell plugin may be used only for an explicitly approved, user-facing feature and must have a narrow capability scope. The kernel execution supervisor is the sole authority for coding tools.

The supervisor must provide:

- generated execution ID, session ID, and run ID;
- explicit workspace root and validated working directory;
- canonical path and symlink-escape checks before mounting or opening files;
- a clean, allowlisted environment rather than inherited `HOME`, `PATH`, SSH, cloud, package-manager, or provider credentials;
- explicit stdin/stdout/stderr handling;
- bounded output with truncation metadata;
- wall-clock timeout, CPU limit, memory limit, process-count limit, and optional disk quota;
- process-group/job-tree termination on cancellation and timeout;
- exit status, signal, timeout, cancellation, and resource-limit classification;
- structured audit events before launch, on approval, on start, on output, and on termination;
- no secret values in command arguments, logs, artifacts, or error messages;
- an explicit network mode: disabled by default, allowlisted when enabled;
- artifact export through a controlled path, never by exposing the worker’s entire filesystem.

#### Required execution profiles

Implement and document these profiles:

1. **Restricted local** — for trusted commands in the user’s own workspace. Use sanitized environment, workspace and path policy, limits, timeout, and process-tree cleanup. Label this mode restricted, not sandboxed.
2. **OS sandbox** — default for model-generated commands. On Linux, evaluate and test a supervisor using user/mount/PID/network namespaces, a read-only runtime root, a workspace bind mount, Landlock where available, seccomp, and cgroups. `bubblewrap` may be used as a launcher, but its configuration is not itself RIGA’s policy and must be generated and audited by the supervisor.
3. **Disposable worker** — for untrusted repositories, install scripts, arbitrary generated binaries, or network-enabled execution. Use a disposable container or microVM with explicit mounts, no host credentials, strict limits, and destruction after artifact export. Hosted/server deployments must use this profile or an equivalent provider isolation boundary.

On macOS and Windows, do not claim Linux namespace semantics. Implement platform-specific workers—such as a disposable VM/container or OS-supported sandbox facility—or fall back to restricted local mode with an explicit warning and disabled high-risk capabilities. The capability is not complete until its isolation strength is reported by `GET /v1/capabilities` and in the desktop UI.

#### Mandatory Tauri v2 configuration

Use Tauri v2 capabilities in `src-tauri/capabilities/`, preferably separate per window and platform. Capabilities grant or deny permissions to windows/webviews, and overlapping capabilities merge their permissions; keep the main window’s privileges minimal. [20]

The Tauri layer must:

- expose only typed commands required by the UI through `invoke_handler`;
- define a command manifest where supported so registered commands are not implicitly available to every window;
- make every command input `serde::Deserialize` and every result/error explicitly serializable;
- return stable `RigaErrorWire` codes, not ad hoc strings or Rust debug output;
- use Tauri `Channel<T>` for ordered, typed high-volume run streaming;
- use ordinary Tauri events only for small, low-throughput notifications, never as the authoritative event journal;
- avoid `eval` for data or control flow;
- keep shell, filesystem, HTTP, notification, updater, process, and window permissions separate and least-privileged;
- configure shell scopes with explicit executable and argument rules if the plugin is enabled;
- keep remote URL capabilities disabled unless a threat model, origin allowlist, authentication design, and review explicitly justify them;
- enable a restrictive CSP and avoid remote scripts/CDN dependencies;
- use the Tauri Isolation Pattern for the frontend IPC threat model when the dependency/embedded-content threat model warrants it, while retaining kernel/OS process isolation;
- test capability files, command scopes, CSP, and command exposure in CI.

Tauri commands are an IPC API, not a security substitute for kernel authorization. Tauri’s scope documentation requires application commands to enforce their own scope checks and warns that deny scopes supersede allow scopes; path and command checks must be audited for bypasses. [22]

#### Mandatory tests and acceptance gates

The implementation is not complete until automated tests demonstrate:

- a frontend cannot invoke an unlisted privileged command;
- a second window does not inherit the main window’s capabilities accidentally;
- shell plugin permissions reject an unscoped executable/argument;
- Tauri command errors serialize into stable RIGA error codes;
- channel events preserve sequence and do not lose terminal status;
- `../`, absolute paths, symlinks, junctions, and workspace mount escapes are rejected;
- provider keys, SSH keys, cloud credentials, and host environment secrets are absent from worker environments;
- timeout and cancellation terminate descendants, not just the shell parent;
- output and resource limits are enforced;
- network-disabled mode cannot reach the network;
- a worker cannot read outside its declared mount/scope;
- disposable workers leave no writable state except exported artifacts;
- capability/CSP configuration passes a release security review;
- the UI clearly reports whether execution is restricted, OS-sandboxed, or disposable.

### 6.4 Desktop failure handling

The app must show an error screen instead of quitting when:

- the kernel cannot initialize;
- the model file is missing;
- CUDA initialization fails;
- the session store is corrupted;
- an event is malformed;
- a tool panics or times out.

Use `catch_unwind` only around explicitly unsafe integration points if needed; do not use it to hide ordinary programming errors. Log with structured tracing and show a user-safe message.

## 7. HTTP/server and Create.xyz adapter

### 7.1 Server endpoints

Implement `riga-server` with a documented OpenAPI contract:

```text
GET  /health
GET  /v1/capabilities
POST /v1/sessions
GET  /v1/sessions
GET  /v1/sessions/{id}
POST /v1/sessions/{id}/messages
POST /v1/runs/{id}/cancel
POST /v1/runs/{id}/approve
GET  /v1/runs/{id}/events?after_sequence=N
GET  /v1/sessions/{id}/artifacts
GET  /v1/artifacts/{id}
```

The server must authenticate users, enforce workspace tenancy, rate-limit runs, and reject unrestricted local filesystem paths. A hosted server must not expose the desktop app’s local workspace by default.

### 7.2 OpenAPI for Create.xyz

Generate `openapi/riga-server.yaml` from the server contract. Test that:

- every endpoint has request/response schemas;
- event schemas include discriminators for `RigaEvent` variants;
- error responses are documented;
- authentication is represented;
- no internal Rust type names leak into the API;
- examples are safe and contain no real secrets.

Import the OpenAPI document into Create.xyz. Build a Create client that uses only the safe server endpoints. Keep generated code as a client layer and commit a small adapter rather than editing generated files manually.

### 7.3 Security for Create-generated apps

- Use short-lived tokens or a backend-for-frontend; do not put provider API keys in the browser.
- Bind a user identity to every session and artifact.
- Use server-side workspace IDs rather than raw paths.
- Require approval for write, shell, package, git-push, and release operations.
- Apply response-size limits and event-stream timeouts.
- Add CORS allowlists, CSRF protection where cookies are used, and abuse rate limits.
- Make public/demo mode read-only by default.

## 8. Testing strategy

### 8.1 Rust tests

`riga-kernel`:

- unit tests for state transitions, path policy, approval policy, event ordering, serialization, and cancellation;
- fake model tests for deterministic agent loops;
- fake tool tests for success, failure, timeout, cancellation, and approval;
- persistence round-trip tests;
- event journal replay tests;
- dependency-hygiene test;
- public API compile test.

Adapters:

- Tauri command serialization tests;
- HTTP endpoint and status/error mapping tests;
- CLI stdout JSON and stderr progress tests;
- byte-level parity tests between dispatcher and every adapter;
- OpenAPI generation and schema validation tests.

### 8.2 React tests

Use Vitest and Testing Library. Cover:

- runtime provider initialization;
- message streaming and event replay;
- tool renderer states;
- approval submit/reject/cancel;
- attachments, mentions, slash commands, model picker, and context display;
- branch/edit/retry behavior;
- thread switching and draft restore;
- connection loss and resume;
- loading/error/empty states;
- keyboard and screen-reader status announcements;
- Tauri and HTTP transport parity;
- no duplicate events after reconnect;
- component rendering against the mock runtime.

### 8.3 Autonomous UI and Manus execution contract

The implementation must be executable by an unattended Manus coding agent. No phase may depend on a developer clicking through a UI, manually editing generated files, or pasting credentials into source or chat.

#### Normal automated path

The coding agent may autonomously:

- inspect and modify the repository;
- install declared dependencies;
- run Rust, Node, Vitest, Testing Library, accessibility, contract, and Playwright tests;
- start and stop local services;
- run a deterministic fake kernel/provider;
- generate screenshots, traces, coverage, OpenAPI, SBOM, and phase reports;
- run secret scanners against the repository and artifacts;
- build CPU-compatible binaries and packages;
- use environment variables supplied by the execution environment without printing them.

The agent must not require a user takeover for ordinary tests. A live OpenAI-compatible provider test is the only expected user-secret surface; the user supplies `OPENCODE_API_KEY` out-of-band or through a protected CI environment.

#### UI automation requirements

Use Playwright in headless mode for the browser-rendered RIGA shell against a deterministic local fake kernel. Add an explicit script such as:

```bash
npm run test:e2e          # headless Playwright, fake kernel, no network
npm run test:e2e:trace    # same suite with trace/screenshots on failure
```

The Playwright suite must cover:

- create/select workspace and session;
- submit a prompt;
- streamed text and tool-call rendering;
- approval request, approve, reject, and stale approval;
- diff and artifact rendering;
- cancellation and timeout states;
- reload, reconnect, replay, and draft restore;
- error, empty, loading, and capability/unavailable states;
- keyboard navigation and core ARIA assertions;
- no duplicate events after reconnect;
- no provider/network access in deterministic mode.

Use Vitest and Testing Library for component and runtime tests. Use Rust/Tauri command contract tests for the native IPC boundary. A native desktop window test may be an additional release smoke test, but the core UI gate must not depend on an interactive display or human clicks: the headless Playwright suite plus Tauri command/channel contract tests are the authoritative automated gate.

#### Manus-compatible isolation strategy

The supervisor must feature-detect available isolation backends. The test suite must include a deterministic `FakeWorker` and a portable restricted worker so kernel, policy, event, timeout, cancellation, and artifact behavior can be tested in an ordinary Manus sandbox without Docker or privileged namespaces. The fake worker is not accepted as evidence of OS security; it only makes application behavior testable.

When an OS backend is available, run conformance tests for mounts, network, credentials, resource limits, and descendant cleanup. When it is unavailable, the phase report must say `OS_BACKEND_UNAVAILABLE` and the build must not claim the OS-sandbox phase passed. The implementation may continue only for phases that do not depend on that gate.

Container/microVM builds, signed installers, GPU/CUDA builds, and protected live-provider tests belong in automated GitHub Actions workflows. They must be triggerable with `workflow_dispatch` or tags and must not require a person to click through the product UI. Code-signing and live-provider secrets are supplied only by protected CI environments.

#### Automated command contract

Every phase should expose a non-interactive command group:

```bash
npm run check              # frontend lint, typecheck, unit tests, build
npm run test:e2e           # headless UI flow with fake kernel
cargo test --workspace     # deterministic Rust tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Commands must use bounded timeouts, fail on unexpected prompts, clean up child processes, and write machine-readable results. A phase is not complete if validation depends on reading a live terminal or manually interpreting a screenshot.

### 8.3 E2E smoke tests

Use a deterministic fake kernel and a test Tauri/web harness to verify:

1. create session;
2. submit prompt;
3. receive streamed text;
4. receive a tool call;
5. show approval card;
6. approve a safe fake write;
7. render diff and artifact;
8. cancel a second run;
9. reload and resume the first session.

Do not use a real provider or real destructive shell command in CI.

## 9. GitHub Actions and publishing

### 9.1 Version source of truth

Use the Git tag as the release source of truth:

```text
v1.2.3
```

A `scripts/set-version.mjs` script writes the version to:

- workspace package metadata;
- `packages/assistant-ui/package.json`;
- `crates/riga-kernel/Cargo.toml`;
- `crates/riga-server/Cargo.toml`;
- `crates/riga-cli/Cargo.toml`;
- `apps/riga/src-tauri/Cargo.toml`;
- `tauri.conf.json`.

The workflow must fail if the tag and package versions disagree.

### 9.2 CI workflow

`.github/workflows/ci.yml` should run on pull requests and pushes to `main`:

1. `frontend` — install, typecheck, lint, build, unit tests, and headless Playwright E2E with the fake kernel.
2. `kernel` — fmt, clippy, unit/integration tests.
3. `adapters` — server, CLI, and Tauri compile checks.
4. `contract` — OpenAPI generation check, TypeScript wire type check, parity tests.
5. `security` — dependency audit, secret scan, capability review, dependency hygiene.
6. `coverage` — enforce separate kernel, adapters, and frontend thresholds.

Use pinned toolchain versions and explicit job timeouts. Follow the Fina Builder practice of keeping CI jobs separate and validating adapter parity and dependency boundaries. [5]

### 9.3 Kernel release workflow

`.github/workflows/release-kernel.yml` triggers on tags matching `v*` or via `workflow_dispatch`.

```yaml
name: Release RIGA kernel

on:
  push:
    tags: ['v*.*.*']

permissions:
  contents: read
  id-token: write

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.99.0
      - uses: actions/setup-node@v4
        with:
          node-version: 22
      - run: node scripts/set-version.mjs "${GITHUB_REF_NAME#v}"
      - run: cargo fmt --all -- --check
      - run: cargo test -p riga-kernel --all-targets
      - run: cargo publish -p riga-kernel --dry-run

  publish:
    needs: verify
    runs-on: ubuntu-latest
    environment: crates-io
    permissions:
      id-token: write
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.99.0
      - run: cargo publish -p riga-kernel
```

Use crates.io trusted publishing/OIDC if available for the selected release setup. Otherwise use a repository secret only as a temporary fallback, with the smallest possible scope and a documented rotation process.

### 9.4 React package release workflow

`.github/workflows/release-ui.yml` should:

- verify the tag;
- install Node with a pinned major version;
- run typecheck, lint, tests, and package build;
- run `npm pack --dry-run` and inspect the file list;
- publish `@riga/assistant-ui` with npm trusted publishing/OIDC or a narrowly scoped token;
- optionally publish to GitHub Packages;
- attach the tarball and generated API report to the GitHub Release.

Never publish source maps containing secrets or local absolute paths. Do publish type declarations, CSS, README, and license files.

### 9.5 Server release workflow

`.github/workflows/release-server.yml` builds:

- Linux x64 binary;
- container image with a pinned base image;
- OpenAPI document;
- SBOM and checksums.

Publish the container to GHCR using `GITHUB_TOKEN` with `packages: write`, sign it where the repository’s supply-chain policy requires, and attach checksums to the GitHub Release.

### 9.6 RIGA desktop release workflow

`.github/workflows/release-desktop.yml` builds Tauri bundles on:

- Ubuntu x64;
- macOS x64;
- macOS arm64;
- Windows x64.

The workflow should:

1. check out the exact tag;
2. install the pinned Rust and Node toolchains;
3. install Tauri system dependencies;
4. build the frontend;
5. build the Tauri app;
6. sign installers if signing secrets are configured;
7. generate checksums;
8. upload artifacts;
9. create or update one GitHub Release with all assets;
10. publish updater metadata only after signature verification.

CUDA builds are optional artifacts and must not block the portable CPU-compatible desktop release. If a CUDA desktop build is offered, build it in a separate workflow with explicit CUDA toolkit and GPU architecture settings.

### 9.7 Release order

Use this order to avoid publishing an app against missing dependencies:

1. run all CI checks;
2. publish `riga-kernel`;
3. publish `@riga/assistant-ui`;
4. publish `riga-server` and OpenAPI;
5. build and publish RIGA desktop bundles;
6. publish Create.xyz integration metadata/client documentation;
7. update release notes with exact artifact versions.

A single release tag can coordinate all artifacts, but each artifact’s workflow should be independently rerunnable and idempotent.

## 10. Automated testing and coverage plan

Testing is a first-class deliverable in every build phase. RIGA should aim for high effective coverage rather than only a high line percentage. The test suite must cover normal behavior, failure behavior, cancellation, security boundaries, persistence, transport parity, and user-facing recovery states.

### 10.1 Test repository layout

```text
crates/riga-kernel/
├── src/**/tests.rs                 # focused unit tests beside implementation
├── tests/
│   ├── public_api.rs              # public API compile/use contract
│   ├── kernel_lifecycle.rs        # create → submit → stream → complete
│   ├── run_state_machine.rs       # valid/invalid transitions
│   ├── event_replay.rs            # journal, cursors, reconnect, dedupe
│   ├── persistence.rs             # restart, migration, corruption
│   ├── tool_policy.rs             # scopes, approvals, deny paths
│   ├── transport_parity.rs        # dispatcher vs adapter output
│   ├── opencode_go_live.rs        # opt-in provider smoke tests
│   └── support/mod.rs             # fake model, fake tools, temp workspace
crates/riga-server/tests/
├── http_contract.rs
├── auth_and_tenancy.rs
├── sse_replay.rs
└── openapi.rs
crates/riga-cli/tests/
├── json_stdout.rs
└── exit_codes.rs
packages/assistant-ui/tests/
├── runtime/
├── components/
├── tools/
├── transports/
└── accessibility/
apps/riga/tests/
├── shell.smoke.test.ts
├── tauri-transport.test.ts
└── e2e/
    ├── riga-flow.spec.ts
    └── playwright.config.ts
```

Rust unit tests belong beside the implementation and may exercise private helpers. Rust integration tests live in `tests/` and use only the crate’s public API, matching Rust’s documented distinction between unit and integration tests. [7]

### 10.2 Unit-test plan

#### Kernel domain and state

Test every public and private branch for:

- request validation and defaulting;
- run state transitions and invalid transitions;
- cancellation idempotency;
- pause/resume semantics;
- retry and fork behavior;
- session/thread/branch operations;
- event sequence allocation and ordering;
- duplicate event suppression;
- protocol-version negotiation;
- error-code mapping and retryability;
- model capability selection;
- tool descriptor validation;
- approval mode evaluation;
- path normalization, symlink escape prevention, and workspace containment;
- command timeout and output truncation;
- artifact hash/metadata generation;
- session ID and correlation ID generation.

Use property-based tests for path policy, event sequence invariants, and state-machine transitions. Add fuzz targets for JSON requests, event envelopes, tool arguments, and persisted session data. A malformed input must return a typed error, not panic.

#### Persistence and recovery

Test:

- empty store initialization;
- atomic write and replace;
- restart after a completed run;
- restart while a run is active;
- replay from sequence zero and from a later cursor;
- truncated/corrupted journal;
- unknown future event type;
- migration from every supported schema version;
- concurrent session writes;
- artifact missing from disk;
- duplicate approval submission.

#### Model/provider configuration

Test without network:

- missing API key;
- whitespace-only API key;
- invalid base URL;
- unsupported protocol family;
- missing model ID;
- configured model override precedence;
- catalog response parsing;
- unknown model handling;
- custom user-agent and session-header construction;
- redaction of the API key from debug/error output.

#### Tools and policy

For every tool, test:

- valid input and result serialization;
- malformed input;
- denied capability;
- approval-required result;
- user rejection;
- cancellation during execution;
- timeout;
- oversized output;
- workspace escape;
- audit event emission;
- retry safety and idempotency where applicable.

### 10.3 Kernel integration tests

Use fake models and fake tools that emit deterministic events. No network, provider secret, or real shell command is allowed in the normal integration suite.

Required scenarios:

1. Create a session, submit a prompt, stream text deltas, and complete.
2. Generate a tool call, pause for approval, approve it, emit the result, and resume the model turn.
3. Reject a tool call and verify the model receives a structured rejection event.
4. Cancel during text streaming and verify no later completion event is emitted.
5. Cancel during a tool call and verify the tool receives cancellation.
6. Disconnect after sequence `N`, reconnect from `N`, and receive exactly the missing events.
7. Restart the kernel and resume a persisted run/session.
8. Produce a diff and artifact, then read it through the public API.
9. Run two sessions concurrently and assert event isolation.
10. Exhaust a policy limit and verify a typed, retryable or non-retryable error as appropriate.
11. Feed malformed provider/tool output and verify safe failure.
12. Execute the same command through the in-process dispatcher and every adapter and compare normalized wire results.

Integration tests should use temporary directories and deterministic clocks where possible. Every test must clean up even after failure.

### 10.4 Adapter and contract tests

#### Tauri

- command request/response serialization;
- channel event serialization;
- frontend cancellation reaches the kernel;
- malformed request maps to the documented error;
- kernel initialization failure becomes an app error state;
- no command performs domain logic outside the kernel;
- capabilities deny unauthorized filesystem/shell access.

#### HTTP/SSE

- authentication and authorization;
- session isolation/tenant isolation;
- request validation and status mapping;
- SSE event ordering and `Last-Event-ID`/cursor replay;
- disconnect and reconnect;
- rate limiting;
- oversized input/output limits;
- CORS and security headers;
- OpenAPI schema matches runtime responses;
- server shutdown does not corrupt an event journal.

#### CLI

- one JSON document on stdout;
- progress only on stderr;
- stable exit codes;
- `--out` file behavior;
- invalid command and invalid JSON behavior;
- secret values never appear in help, error, or debug output.

#### Transport parity

Create a single fixture for each command. Run it through:

```text
dispatcher → Tauri adapter → HTTP adapter → CLI adapter
```

Compare response JSON, error `{ code, message, retryable, details }`, event sequence, and cancellation behavior. Do not parse each result into a different model and compare only selected fields; byte-level or canonical-JSON parity catches adapter drift.

### 10.5 React and assistant-ui component tests

Use a deterministic `MockRigaRuntime` and Testing Library. Cover:

- welcome/empty state;
- history loading and earlier-message loading;
- streamed assistant text;
- reasoning/tool-call rendering;
- tool running, complete, error, cancelled, and approval-required states;
- approval submit, reject, and stale approval;
- composer send, cancel, edit, retry, branch, draft restore;
- attachments add/remove/reject/upload progress;
- mentions and slash commands;
- model picker and context meter;
- thread list creation, switching, archive, and delete;
- connection lost/reconnecting/resumed;
- duplicate event replay;
- plan/checkpoint/background-run updates;
- code diff rendering and file selection;
- command runner output, exit code, timeout, and rerun;
- artifact download and missing artifact;
- error boundary and retry;
- theme, keyboard navigation, focus retention, and ARIA live announcements.

Every component with asynchronous data must be tested in `idle`, `loading`, `ready`, `error`, and `empty` states. Use `axe`/Testing Library accessibility assertions for critical flows. Do not rely only on snapshots; interaction assertions are required.

### 10.6 Desktop and end-to-end tests

Run deterministic end-to-end smoke tests against a fake kernel:

1. Launch RIGA.
2. Create or select a workspace.
3. Create a session.
4. Send a coding prompt.
5. Observe streamed text and plan state.
6. Trigger a fake file-write tool.
7. Approve it from the UI.
8. Render the diff and artifact.
9. Cancel a second run.
10. Reload the app and resume the first session.
11. Disconnect/reconnect the transport and verify no duplicated events.
12. Close and reopen the app and verify persisted thread state.

Keep real-model, real-shell, and destructive filesystem operations out of CI E2E tests.

### 10.7 Coverage and mutation gates

Install `cargo-nextest` and `cargo-llvm-cov`. The nextest documentation supports coverage collection through `cargo llvm-cov nextest`; CI should also merge doctest coverage separately if doctests are part of the release gate. [8]

Recommended commands:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features
cargo test --workspace --doc
cargo llvm-cov nextest --workspace --all-features --lcov --output-path coverage/rust.lcov
cargo llvm-cov report --summary-only
npm run lint
npm run typecheck
npm run test:run -- --coverage
```

Initial gates:

- kernel domain/state/policy: 90% line coverage;
- kernel persistence/events: 85%;
- adapters: 85%;
- server security/auth: 90%;
- CLI: 80%;
- Tauri commands: 80%;
- assistant-ui runtime/protocol: 90%;
- critical React components: 85%;
- overall frontend: 80%.

Add mutation testing with `cargo mutants` for policy, state transitions, error mapping, path safety, and event replay. Mutation jobs may run nightly or on release candidates rather than every pull request, but a release candidate must have no surviving mutants in safety-critical modules without an explicitly documented reason.

### 10.8 Fuzzing and stress testing

Add `cargo-fuzz` targets for:

- `KernelCommand` JSON decoding;
- `RigaEventEnvelope` decoding;
- tool argument schemas;
- persisted session/journal recovery;
- workspace path normalization.

Add stress tests for:

- many concurrent sessions;
- rapid cancel/resume;
- reconnect loops;
- large tool output;
- event journal growth and compaction;
- simultaneous artifact reads.

Run bounded stress tests nightly. Fail on panic, deadlock, event loss, event reordering, or unbounded memory growth.

## 11. OpenCode Go integration-test plan

Rig’s OpenAI provider supports `base_url` for OpenAI-compatible services, so RIGA should use the Rig OpenAI client with a custom endpoint rather than inventing a provider implementation where the protocol matches. [8] OpenCode Go’s official documentation currently lists model-specific endpoint families and asks compatible clients to send a stable `x-opencode-session` header plus an identifying user agent. [10]

### 11.1 Configuration

```text
OPENCODE_API_KEY       required only for live tests
RIGA_OPENCODE_BASE_URL default: https://opencode.ai/zen/go/v1
RIGA_OPENCODE_MODEL    explicit bare model ID; do not add opencode/ prefixes
RIGA_OPENCODE_PROTOCOL optional: chat-completions | responses | anthropic
RIGA_LIVE_LLM          set to 1 to enable live tests
RIGA_LIVE_LLM_TIMEOUT  default: 45s
```

The key is supplied out-of-band by the user or as a protected GitHub Actions environment secret. It must never be placed in:

- source files;
- `.env` files committed to Git;
- test fixtures;
- command-line arguments;
- issue comments or chat messages;
- CI logs, snapshots, artifacts, or failure reports.

Local execution:

```bash
export OPENCODE_API_KEY='provided-out-of-band'
export RIGA_LIVE_LLM=1
export RIGA_OPENCODE_MODEL='configured-current-model'
cargo test -p riga-kernel --test opencode_go_live -- --nocapture
```

The test harness must fail closed if `RIGA_LIVE_LLM=1` but the key is absent. If the opt-in flag is absent, the test should skip with an explicit message rather than fail the normal suite.

### 11.2 Model and protocol discovery

Do not make `big-pickle` a hardcoded required model. OpenCode’s catalog may change. Use this selection order:

1. `RIGA_OPENCODE_MODEL`;
2. a release-pinned model ID from a non-secret test configuration;
3. a live `/models` catalog lookup when available;
4. a clear skipped/unsupported result.

OpenCode’s current endpoint table shows that some Go models use `/chat/completions`, some `/responses`, and some `/messages`. [10] The model configuration must therefore carry a protocol family. A Chat Completions test must select a model documented for Chat Completions; it must not send a Chat Completions payload blindly to a Responses-only model.

### 11.3 Rig provider construction

The preferred implementation is conceptually:

```rust
let client = rig::providers::openai::Client::builder()
    .api_key(api_key)
    .base_url(base_url)
    .build()?;

let model = client.completion_model(model_id);
```

The coding agent must confirm the exact API for the pinned Rig release. Add a small provider-construction unit test that verifies:

- the base URL is not silently changed;
- the model ID remains bare;
- the API key is not included in `Debug` output;
- the session ID and user agent are attached to requests.

If the Rig builder cannot set custom headers, implement a narrow RIGA provider adapter using the same OpenAI-compatible request/response schema or extend the upstream client behind a compatibility module. Do not omit `x-opencode-session`. Use `riga/<version>` as the user agent.

### 11.4 Live smoke tests

The live suite should include only low-cost, bounded tests:

1. **Connectivity/auth** — send a minimal prompt and assert a successful structured response.
2. **Catalog/configuration** — verify the configured model is discoverable or return a clear unsupported-model result.
3. **Streaming** — request a short response and assert ordered deltas plus one terminal event.
4. **Coding-agent prompt shape** — send a small coding task and assert non-empty text, without exact prose matching.
5. **Tool schema compatibility** — only for a model/protocol documented as tool-capable; assert a structural tool call or a valid refusal.
6. **Session header** — use a mock HTTP capture layer for header assertions; do not attempt to infer headers from provider output.
7. **Timeout/cancellation** — use a local mock server for deterministic cancellation; do not spend provider quota to test cancellation.

Never use live tests for exact generated code, exact token counts, exact reasoning text, or deterministic business calculations.

### 11.5 CI policy for live tests

Normal pull requests run all deterministic tests and skip live provider tests. A protected manual workflow may run the live suite:

```yaml
name: Live OpenCode Go smoke test

on:
  workflow_dispatch:
    inputs:
      model:
        required: false
        type: string
        description: Optional current OpenCode Go model ID

jobs:
  live-opencode:
    runs-on: ubuntu-latest
    environment: opencode-live
    permissions:
      contents: read
    env:
      OPENCODE_API_KEY: ${{ secrets.OPENCODE_API_KEY }}
      RIGA_LIVE_LLM: '1'
      RIGA_OPENCODE_MODEL: ${{ inputs.model }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.99.0
      - run: cargo test -p riga-kernel --test opencode_go_live -- --nocapture
```

Use a protected environment with approval, restrict the workflow to trusted branches, and do not run it for fork pull requests. GitHub’s secret guidance states that secrets are not passed to fork-triggered workflows and recommends environment variables rather than command-line arguments. [9] Never print the environment or request headers.

### 11.6 Cost, quota, and failure handling

- Use one or two short requests per live run.
- Set a hard timeout and bounded retries.
- Treat HTTP 401, 403, 404, 429, 5xx, timeout, and malformed response as distinct diagnostics.
- Do not automatically retry authentication failures.
- Do not turn provider quota failures into application regressions.
- Emit a redacted summary containing endpoint host, protocol family, model ID, elapsed time, and error category only.
- Keep provider integration tests separate from coverage gates because network availability and model behavior are not deterministic.

## 12. Phased implementation roadmap and hard gates

The coding agent must implement **one phase at a time**. It must not begin the next phase until the current phase’s exit criteria are demonstrated in CI or in a recorded local verification report. A phase may add no more than the scope listed below unless the change is documented as an ADR.

Every phase produces:

1. code and tests;
2. a short `docs/phase-N-report.md` containing commands run, results, known limitations, and artifact links;
3. updated API/schema documentation;
4. a clean working tree and reviewable commit.

A failed gate blocks the next phase. Never bypass a gate by weakening a test or silently changing the acceptance criterion.

### Phase 0 — Repository, naming, and dependency lock

**Goal:** establish a reproducible RIGA workspace without implementing agent behavior.

**Deliverables:**

- RIGA package/crate naming migration;
- workspace layout and package boundaries;
- pinned Rust, Node, pnpm/npm, Tauri, Rig, and assistant-ui versions;
- dependency licenses and supply-chain policy;
- CI skeleton for format, lint, typecheck, unit tests, and Rust compilation;
- empty `rig_compat` module and version-compatibility test scaffold;
- first versioned wire-schema directory.

**Exit gate:**

- fresh checkout succeeds with documented bootstrap commands;
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `npm run lint`, and `npm run typecheck` pass;
- `riga-kernel` compiles without Tauri, browser, or HTTP-server dependencies;
- `cargo tree`/dependency-hygiene check confirms the boundary;
- no secrets are present in tracked files;
- a phase report records exact toolchain versions.

### Phase 1 — Rig compatibility layer and fake vertical slice

**Goal:** prove the real pinned Rig APIs and delegate execution to Rig rather than inventing an agent loop.

**Deliverables:**

- `rig_compat` provider/model construction;
- minimal Rig agent/runner construction using the pinned release;
- Rig tool registration with one deterministic fake tool;
- stream-item conversion into `RigaEventEnvelope`;
- Rig hook registration for audit/event translation;
- fake model and scripted multi-turn test;
- initial `RigaKernel::submit` and stream API.

**Exit gate:**

- a fake Rig model completes a prompt and a scripted tool-call turn;
- the kernel emits ordered `RunStarted`, text/tool, and terminal events;
- no RIGA-owned duplicate model/message/tool-loop abstraction exists;
- the compatibility test compiles against the pinned Rig version;
- tests cover model success, tool success, tool failure, invalid tool call, and max-turn termination;
- the phase report includes the exact Rig APIs used and any version caveats.

### Phase 2 — Domain state, persistence, and replay

**Goal:** make runs durable and resumable before adding UI complexity.

**Deliverables:**

- session/thread/branch model;
- run state machine and typed transitions;
- durable session store;
- event journal with sequence numbers;
- checkpoint and artifact metadata;
- cancellation, pause, resume, retry, and fork semantics;
- replay/deduplication logic.

**Exit gate:**

- persistence round-trips survive process restart;
- replay from `after_sequence` reconstructs the same state;
- duplicate and out-of-order events are rejected or deduplicated deterministically;
- cancellation cannot be followed by completion;
- invalid transitions and unknown IDs return stable error codes;
- crash-recovery tests pass with truncated/corrupt journal fixtures;
- kernel domain/persistence coverage meets the phase threshold: 90% line coverage for state/policy and 85% for persistence/events.

### Phase 3 — Tauri v2 adapter and transport proof

**Goal:** connect the kernel to a desktop WebView without moving domain logic into Tauri commands.

**Deliverables:**

- Tauri v2 application shell;
- managed `Arc<RigaKernel>` state;
- typed commands for session, submit, stream, cancel, approve, resume, and artifact reads;
- typed `Channel<T>` run streaming;
- stable serialized `RigaErrorWire` responses;
- capability files and minimal window permissions;
- actionable initialization/error boundary;
- CSP and frontend IPC configuration.

**Exit gate:**

- a fake run can start, stream, cancel, reload, and resume through Tauri;
- command handlers contain no tool execution, model selection, path-policy, or domain logic;
- channel events preserve order and terminal status;
- malformed requests return typed errors without crashing the app;
- capability tests prove unlisted privileged commands are unavailable;
- `cargo test`, frontend tests, and a Tauri smoke test pass;
- the desktop app stays open when the kernel, model, or tool returns an error.

### Phase 4 — assistant-ui foundation and runtime parity

**Goal:** implement the user-facing thread experience against the stable runtime contract.

**Deliverables:**

- runtime provider/transport adapter;
- thread, composer, thread list, welcome, loading, and reconnect states;
- message streaming and replay;
- approval and cancellation controls;
- tool-call renderers and error states;
- mock, Tauri, and HTTP runtime adapters;
- accessibility and theme tokens.

**Exit gate:**

- the same recorded session renders identically enough across mock, Tauri, and HTTP adapters;
- no React component owns authoritative agent state;
- streaming, reconnect, resume, cancellation, error, empty, and approval states are tested;
- keyboard and screen-reader checks pass for the core flow;
- component and runtime coverage meets 90% runtime and 85% critical-component targets.

### Phase 5 — Coding tools, policy, and restricted execution

**Goal:** implement useful coding actions with kernel-side authorization before introducing OS sandboxing.

**Deliverables:**

- read/search/list tools;
- scoped write and apply-patch tools;
- diff/checkpoint/artifact tools;
- restricted command supervisor;
- git inspection tools;
- approval policy and audit log;
- canonical path, symlink, environment, output, timeout, and process-tree policies;
- tool cards for running, approval-required, success, failure, and cancellation.

**Exit gate:**

- every privileged operation is policy-checked in the kernel;
- path traversal, symlink/junction escape, unauthorized workspace, and secret leakage tests pass;
- cancellation kills descendants, not only the shell parent;
- restricted execution reports its weaker isolation level visibly;
- fake end-to-end flow can read, patch, approve, render a diff, create an artifact, and cancel a command;
- no frontend shell-plugin call is used for agent commands.

### Phase 6 — OS sandbox and disposable worker

**Goal:** provide actual process isolation for model-generated and untrusted workloads.

**Deliverables:**

- Linux OS-sandbox worker with explicit mounts, namespaces, read-only runtime, network policy, Landlock/seccomp evaluation, and cgroup limits;
- platform-specific macOS/Windows strategy or disposable VM/container fallback;
- disposable container/microVM worker;
- artifact export/import boundary;
- isolation-strength capability reporting;
- supervisor conformance tests.

**Exit gate:**

- worker cannot read outside declared mounts;
- network-disabled mode cannot reach the network;
- provider, SSH, cloud, and host credentials are absent;
- resource limits and process-count limits are enforced;
- worker termination cleans descendants and temporary state;
- disposable mode leaves only explicitly exported artifacts;
- the UI and `GET /v1/capabilities` report restricted, OS-sandboxed, or disposable mode;
- security review accepts the platform-specific fallback behavior.

### Phase 7 — HTTP, OpenAPI, and Create.xyz adapter

**Goal:** expose only the safe, authenticated server surface.

**Deliverables:**

- `riga-server` HTTP adapter;
- authentication, authorization, tenancy, rate limits, and CORS policy;
- session/run/artifact/approval endpoints;
- replayable event streaming;
- generated OpenAPI contract;
- safe Create.xyz client proof;
- hosted worker isolation configuration.

**Exit gate:**

- OpenAPI schema validation and contract tests pass;
- unauthenticated, cross-tenant, raw-path, unrestricted-shell, and secret-exposure tests fail closed;
- Create.xyz can run a restricted demo flow through the API;
- browser code contains no provider key or local filesystem authority;
- server uses disposable or equivalent isolated workers for hosted execution.

### Phase 8 — OpenCode Go live compatibility

**Goal:** validate one real OpenCode Go integration without making live networking part of normal CI.

**Deliverables:**

- Rig OpenAI-compatible configuration with the Go endpoint;
- model/protocol discovery;
- session header and user-agent handling;
- opt-in live tests using `OPENCODE_API_KEY` only from the environment;
- mock HTTP tests for headers, cancellation, timeout, and protocol selection;
- redacted diagnostics.

**Exit gate:**

- normal tests pass without a key or network;
- protected live workflow passes connectivity/auth and short streaming smoke tests when enabled;
- model IDs remain bare and protocol families are respected;
- no key appears in source, logs, snapshots, artifacts, or reports;
- live tests use structural assertions, not exact generated prose.

### Phase 9 — Hardening, coverage, packaging, and release

**Goal:** make the system releasable and integrable by a second Rust/Tauri host.

**Deliverables:**

- fuzzing and bounded stress tests;
- mutation testing for safety-critical policy/state/path/event modules;
- SBOM, license report, checksums, signed desktop bundles;
- crates.io/npm/server/container/GitHub Release workflows;
- rollback and incident runbook;
- second-host integration example;
- complete documentation and migration notes.

**Exit gate:**

- all deterministic CI jobs pass from a clean checkout;
- coverage gates pass and safety-critical mutation survivors are reviewed;
- fuzz/stress runs show no panic, deadlock, event loss, reordering, or unbounded growth;
- aligned release artifacts are produced from a clean tag;
- a second Tauri/Rust host embeds `riga-kernel` successfully;
- the full 12-step desktop E2E scenario passes with fake tools;
- release notes distinguish CPU-compatible and optional CUDA artifacts.

### Phase gate protocol for coding agents

At the end of every phase, the coding agent must output:

```text
Phase: N
Status: PASS | BLOCKED
Implemented:
Tests and commands:
Artifacts:
Known limitations:
Next phase allowed: YES | NO
```

The agent must mark the phase **BLOCKED** when an exit criterion is not met. It must not proceed by treating a warning, skipped security test, missing platform implementation, or unverified Rig/Tauri API as a pass.

## 13. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Rig API changes | Pin a version, isolate upstream types, compile a minimal agent/runner compatibility proof first, and use a compatibility module. |
| UI/runtime mismatch | Use the assistant-ui runtime contract and transport parity tests. |
| Tauri crash on model/tool failure | Return typed errors, guard initialization, cancel safely, and test malformed events. |
| Shell/file privilege escalation | Kernel-side scopes, approvals, symlink checks, timeouts, and audit events. |
| Create.xyz generated client exposes secrets | Backend-for-frontend, short-lived tokens, no provider keys in browser. |
| Event loss on reconnect | Sequence numbers, durable journal, replay endpoint, and client deduplication. |
| Package release drift | Tag-derived versioning and a workflow that verifies all package versions. |
| CUDA-specific build failures | Keep CPU release independent; pin one toolkit and architecture for optional CUDA artifacts. |
| Large assistant-ui surface | Implement in phases, begin with foundation/thread/tool UI, and keep public exports small. |

## 14. Definition of done

RIGA is ready for a first public release when:

- a fresh checkout passes all CI jobs;
- `riga-kernel` can be embedded by another Rust/Tauri application;
- `@riga/assistant-ui` renders the same session through mock, Tauri, and HTTP runtimes;
- Tauri commands contain no domain business logic;
- every privileged tool is policy-checked and auditable;
- events are resumable after frontend reload or connection loss;
- Create.xyz can consume the restricted OpenAPI server contract;
- GitHub Actions publishes aligned kernel, UI, server, and desktop artifacts;
- release notes clearly distinguish CPU-compatible and optional CUDA artifacts;
- documentation contains an integration example for a second Tauri/Rust host.

## References

[1]: https://www.assistant-ui.com/elements "assistant-ui Elements catalog"
[2]: https://www.assistant-ui.com/docs/runtimes/custom/overview "assistant-ui Custom Runtime overview"
[3]: https://www.assistant-ui.com/elements/thread "assistant-ui Thread element and primitive composition"
[4]: https://www.assistant-ui.com/docs/tools/defining-tools "assistant-ui external tools and tool rendering"
[5]: https://raw.githubusercontent.com/haymant/fina-builder/main/FEATURES.md "Fina Builder architecture, adapter boundaries, tests, and release workflow"
[6]: https://create.xyz/how-it-works "Create.xyz REST API, export, and publishing capabilities"

[7]: https://doc.rust-lang.org/book/ch11-03-test-organization.html "The Rust Programming Language — Test Organization"
[8]: https://rig.rs/docs/integrations/model_providers/openai "Rig OpenAI provider and OpenAI-compatible base URLs"
[9]: https://docs.github.com/en/actions/security-for-github-actions/security-guides/using-secrets-in-github-actions "GitHub Actions secret handling"
[10]: https://opencode.ai/docs/go/ "OpenCode Go endpoints, models, and session headers"
[11]: https://github.com/0xPlaygrounds/rig "Rig repository architecture and runtime choices"
[12]: https://rig.rs/docs/concepts/agent "Rig agents and the built-in agent loop"
[13]: https://rig.rs/docs/concepts/completion "Rig completion abstractions and provider-neutral model layer"
[14]: https://rig.rs/docs/concepts/hooks "Rig hooks, flow actions, and guardrail boundaries"
[15]: https://rig.rs/docs/concepts/testing "Rig mock models and deterministic testing"
[16]: https://rig.rs/docs/concepts/memory "Rig conversation memory and memory policies"
[17]: https://rig.rs/docs/guides/extension/write_your_own_provider "Rig custom provider extension guide"
[18]: https://github.com/tauri-apps/tauri "Tauri architecture and repository"
[19]: https://v2.tauri.app/security/ "Tauri security model and trust boundaries"
[20]: https://v2.tauri.app/security/capabilities/ "Tauri v2 capabilities and permissions"
[21]: https://v2.tauri.app/plugin/shell/ "Tauri shell plugin and command scopes"
[22]: https://v2.tauri.app/security/scope/ "Tauri command scopes and enforcement"
[23]: https://v2.tauri.app/concept/inter-process-communication/isolation/ "Tauri IPC Isolation Pattern"
[24]: https://v2.tauri.app/security/csp/ "Tauri Content Security Policy"
[25]: https://v2.tauri.app/develop/calling-frontend/ "Tauri events and typed channels"
[26]: https://v2.tauri.app/develop/calling-rust/ "Tauri commands and serializable errors"
