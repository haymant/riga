# RIGA — Product Architecture and Requirements Plan

> **Status:** Initial design plan
>
> **Brand:** RIGA
>
> **Previous working name:** `rig-coding-app`
>
> **Scope:** Design and planning only. This document is intended to be handed to a coding agent for implementation.

## 1. Product definition

RIGA is a desktop-first coding-agent application built with Tauri, React, TypeScript, Rust, and the `rig-agent` runtime. RIGA is deliberately thin at the application layer. The reusable value lives in two independently consumable packages:

1. `riga-kernel` — a Rust coding-agent kernel that owns sessions, tools, model execution, event streams, persistence, permissions, and recovery.
2. `@riga/assistant-ui` — a React component package that presents the kernel through assistant-ui conventions and can be embedded in RIGA or another Tauri/Rust application.

The `riga` desktop application supplies only composition, platform integration, configuration, and release packaging. It must not become the place where agent business rules accumulate.

### 1.1 Goals

- Provide a polished coding-agent experience for local Tauri desktop use.
- Make the agent kernel embeddable in other Rust/Tauri applications.
- Make the React UI embeddable in other React applications.
- Support local models, remote model providers, MCP tools, and application-owned tools behind one event protocol.
- Support streaming responses, tool calls, approvals, code diffs, command execution, artifacts, and resumable sessions.
- Keep Tauri, HTTP, CLI, WebSocket, and future integrations as mechanical adapters over the same kernel API.
- Publish the kernel and UI as versioned artifacts with GitHub Actions.
- Make RIGA consumable by Create.xyz through a secure REST/OpenAPI adapter.

### 1.2 Non-goals

- RIGA is not a general-purpose IDE replacement in the first release.
- The kernel does not contain React, Tauri, browser, HTTP-server, or UI dependencies.
- The React package does not execute arbitrary shell commands or hold provider secrets.
- Create.xyz is not treated as a Rust crate registry or as a replacement for GitHub Releases, crates.io, or npm.
- The first release does not promise collaborative multi-user editing or remote execution without an explicit server product.

## 2. Naming and package identity

The product and desktop binary are branded **RIGA**. The old application name `rig-coding-app` must not appear in user-facing labels, package descriptions, window titles, documentation headings, or release names.

Recommended identifiers:

| Layer | Identifier |
|---|---|
| Product | RIGA |
| Tauri application | `riga` |
| Desktop binary | `riga` |
| Rust kernel crate | `riga-kernel` |
| Rust kernel library | `riga_kernel` |
| React package | `@riga/assistant-ui` |
| Optional transport package | `@riga/transport-tauri` |
| Optional server adapter crate | `riga-server` |
| Repository | `riga` or `riga-platform` |
| GitHub release tag | `vMAJOR.MINOR.PATCH` |

`rig-agent` remains the upstream runtime dependency name. Do not rename upstream dependencies merely for branding.

## 3. Architectural principles

### 3.1 Transport is not business logic

Every adapter performs only:

```text
deserialize → authenticate/authorize → call riga-kernel → serialize/stream
```

A Tauri command, HTTP route, CLI command, or WebSocket handler must not contain a domain default, tool policy, agent loop, model selection rule, file mutation rule, or coding-agent formula. The Fina Builder architecture establishes this boundary explicitly: all business logic belongs in a transport-free kernel, while Tauri and HTTP are thin adapters. [5]

### 3.2 The kernel owns the application run lifecycle around Rig

The kernel owns session identity, run identity, cancellation, event ordering, persistence, recovery, policy, and the Rig runtime instance. Rig owns the model/tool execution loop. The kernel must coordinate that loop without reimplementing it. Tauri owns the process and window. React owns presentation and user interaction. The browser never receives an `Agent` object; it receives serializable snapshots and events.

### 3.3 Serializable wire contracts

All boundaries use versioned, JSON-compatible request, response, and event types. Rust domain types may be richer internally, but the public wire contract must be stable and documented. The contract must include a protocol version and correlation IDs.

### 3.4 Explicit authority

Coding agents can read files, write files, run commands, invoke network tools, and change a project. Each capability is represented as a named tool with a policy, scope, approval requirement, and audit record. Consequential actions require a human approval UI and a kernel-side authorization check. A UI-only approval is not sufficient.

### 3.5 One source of truth across transports

Tauri IPC, local HTTP, remote HTTP, CLI, and MCP-facing adapters must call the same kernel dispatcher. Transport parity tests must compare serialized results and error shapes.

### 3.6 Rig reuse boundary: do not reinvent the agent runtime

RIGA uses **Rig as the LLM application runtime**. RIGA is not another model/provider abstraction, tool-calling loop, streaming framework, or memory framework.

Rig currently provides the provider-neutral foundation in `rig-core` and the classic agent runtime in `rig-agent`. The official repository describes `rig-core` as containing provider-neutral messages, completion models, portable/contextual tool contracts, memory and vector-store contracts, and provider mappings. `rig-agent` contains the classic agent builder, prompt and streaming traits, typed hooks, live tool registry, extraction, and a serializable `AgentRun` state machine. [11]

### What Rig provides to RIGA

| Capability | Rig responsibility | RIGA responsibility |
|---|---|---|
| Provider clients | OpenAI, Anthropic, Gemini, Cohere, and other provider clients | Provider selection UI, config validation, secret storage, product defaults |
| OpenAI-compatible endpoints | OpenAI client with configurable `base_url` and model ID | OpenCode Go configuration, protocol selection, session/user-agent headers if the Rig client does not expose them |
| Completion model | `CompletionModel`, request builders, normalized messages/content, usage, typed errors | Map normalized results into the RIGA wire/event contract |
| Agent loop | Prompt → model → tool calls → tool results → repeat, `max_turns`, invalid-tool handling | Run lifecycle, durable journal, app cancellation, approvals, audit, UI state |
| Tools | `Tool` trait, schemas, argument deserialization, `call`, tool-result feedback | Concrete coding tools, workspace scope, authorization, approval workflow, audit records |
| Streaming | `StreamingPrompt`, `StreamingChat`, `StreamingCompletion`, multi-turn stream items, text/tool deltas, final usage | Backpressure-aware forwarding to Tauri/HTTP/SSE and assistant-ui events |
| Hooks | `AgentHook`, `StepEvent`, `Flow` for observation, request shaping, tool veto/rewrite, retry/repair/skip | Register RIGA audit/telemetry/approval hooks; enforce security again inside tools and policy services |
| Conversation history | `ConversationMemory`, in-memory memory, load/append/clear contract, history shaping companions | Durable session store, user-visible threads/branches, migrations, retention, encryption, product semantics |
| RAG/vector stores | Embeddings, vector-store contracts, dynamic context/tools, companion integrations | Index workspace/project data, consent, indexing jobs, UI, tenancy, invalidation, access policy |
| Structured output | `TypedPrompt`, extractors, `serde`/schema-based parsing | Domain schemas, validation policy, typed tool/artifact commands, UI renderers |
| Testing model | `MockCompletionModel`, `MockTurn`, `MockEmbeddingModel`, request inspection, cassette replay | RIGA kernel/adapter/UI tests, fake tools, policy tests, transport parity, end-to-end tests |
| Observability | Usage and provider/runtime events, hooks, tracing integrations | Run IDs, event journal, redaction, audit log, metrics export, user-facing activity timeline |
| Multi-agent composition | Agents can be exposed as tools for manager-worker patterns | Product-level delegation, permissions, budgets, workspace ownership, UI handoff/checkpoints |

Rig’s agent loop already builds requests from preamble, context, history, and tool definitions; executes tool calls; feeds results back; and repeats until a final response or turn limit. [12] RIGA must call and observe that loop rather than implement a second loop in `riga-kernel`.

### RIGA code that must not duplicate Rig

Do not build RIGA-specific replacements for:

- `CompletionModel` or provider-neutral message types;
- OpenAI/Anthropic request serialization when an existing Rig provider supports the protocol;
- `AgentBuilder`/`AgentRunner`/`AgentRun` behavior;
- `Prompt`, `Chat`, `TypedPrompt`, or streaming trait behavior;
- tool argument JSON parsing and ordinary tool-result round trips;
- generic `max_turns` and invalid-tool-call recovery;
- generic conversation-memory traits;
- generic embedding/vector-store abstractions;
- generic token-usage extraction;
- mock completion/embedding models;
- generic provider error types;
- ordinary agent hooks and flow actions.

RIGA may wrap these APIs with an application-facing interface, but the wrapper must delegate and preserve semantics. Any intentional divergence requires an ADR explaining why Rig cannot provide the required behavior.

### RIGA code that Rig does not provide

RIGA must implement the product layer Rig intentionally leaves to applications:

- desktop lifecycle and Tauri commands;
- REST/SSE/CLI transports and protocol versioning;
- durable multi-session persistence and event replay;
- workspace-root and symlink security;
- command execution sandboxing;
- approval UX and authoritative authorization;
- coding tools such as read/search/write/patch/run/git/artifact;
- tool capability catalog and product policy;
- file diffs, checkpoints, artifacts, and project context;
- assistant-ui runtime and components;
- user-facing thread/branch/workspace semantics;
- application retries, reconnect, resume, and crash recovery;
- provider secret storage and redaction;
- OpenCode Go-specific configuration and live-test workflow;
- release packaging for RIGA, `riga-kernel`, and `@riga/assistant-ui`.

A Rig hook is useful for observing or steering a run, but Rig’s own documentation states that a hook guardrail is not a security boundary. RIGA must enforce authorization inside the tool implementation or downstream service as well. [14]

### Recommended RIGA runtime composition

The first implementation should use this composition:

```text
RigaKernel
  ├── Rig provider client + CompletionModel
  ├── Rig AgentBuilder / AgentRunner / AgentRun
  ├── Rig Tool implementations wrapped by RigaPolicyTool
  ├── Rig AgentHook stack for audit, event translation, and approval pause
  ├── Riga durable SessionStore / EventJournal
  └── Riga transport adapters
```

`RigaKernel` coordinates application concerns around a Rig agent. It should not claim to be an alternative agent runtime. If the current Rig version exposes `AgentRun` as a serializable sans-I/O state machine, prefer persisting and resuming that state rather than inventing a parallel RIGA run state machine. [14]

### Version and extension rule

Pin a Rig release and isolate all Rig imports in a small compatibility module. Rig’s repository warns that the project is evolving and may contain breaking changes. [11] The compatibility module should own:

- provider/client construction;
- agent builder construction;
- stream-item conversion;
- hook registration;
- memory adapter construction;
- mock-model helpers.

Only implement a custom Rig provider when the target protocol cannot be represented by an existing provider. For an OpenAI-compatible service, first use Rig’s OpenAI client `base_url`; implement `CompletionClient`/`CompletionModel` only when endpoint semantics, headers, or response formats genuinely require it. [8] [17]

## 4. Layered system

```text
┌───────────────────────────────────────────────────────────────┐
│ RIGA desktop application                                      │
│ Tauri window, menus, filesystem paths, updater, permissions   │
│ Thin commands and event forwarding only                       │
└──────────────────────────────┬────────────────────────────────┘
                               │ Tauri IPC / Channel
┌──────────────────────────────▼────────────────────────────────┐
│ @riga/assistant-ui                                             │
│ assistant-ui runtime/provider, Thread, Composer, tool UI      │
│ file tree, diff, terminal, approvals, artifacts, workspaces    │
└──────────────────────────────┬────────────────────────────────┘
                               │ typed RIGA transport
┌──────────────────────────────▼────────────────────────────────┐
│ riga-kernel                                                     │
│ sessions · Rig runtime adapter · model registry · tools        │
│ policy · approvals · persistence · event journal · recovery    │
│ no Tauri · no React · no HTTP · no browser dependencies        │
└───────────────┬──────────────────────┬────────────────────────┘
                │                      │
       ┌────────▼────────┐    ┌────────▼─────────┐
       │ rig-core /       │    │ RIGA coding-tool  │
       │ rig-agent runtime│    │ implementations   │
       └─────────────────┘    └───────────────────┘
```

The same kernel can be embedded by:

- a Tauri app;
- a local Axum/Actix server;
- a CLI;
- a test harness;
- a future remote agent service;
- a Create.xyz-generated frontend using the public REST/OpenAPI adapter.

## 5. React experience requirements

The React package must follow assistant-ui’s runtime-first convention. assistant-ui describes `Thread` as a complete chat surface whose messages, composer, auto-scroll, welcome, history-loading, and running states come from the runtime rather than from manually supplied props. [1] RIGA must therefore expose a runtime adapter and compose assistant-ui primitives instead of building a parallel chat state machine.

### 5.1 Runtime boundary

The default integration is an `AssistantTransport`-style adapter because RIGA has rich agent state: tool calls, approvals, file changes, command output, checkpoints, artifacts, and resumable runs. assistant-ui documents `AssistantTransport` for agents that stream structured state snapshots and need bidirectional commands beyond ordinary message submission. [2]

The package should also provide:

- a local in-memory runtime for component tests and demos;
- a REST/SSE runtime for browser and Create.xyz clients;
- a Tauri channel runtime for the desktop app;
- an external-store integration for host applications that already own Zustand, Redux, or TanStack state.

The UI package must not assume that the backend is OpenAI, Vercel AI SDK, LangGraph, or any other specific provider.

### 5.2 Required assistant-ui elements

RIGA should use or adapt the following element families from the assistant-ui catalog. The implementation phase must verify the current package/API names against the pinned assistant-ui version.

#### Core conversation

- Thread shell with welcome, history loading, empty, error, running, and auto-scroll states.
- Message list with assistant, user, system, and tool messages.
- Composer with send, cancel, retry, edit, and draft restore.
- Thread list and thread switching.
- Branch picker for regenerated or edited responses.
- Markdown text renderer with code blocks, tables, links, and safe external-link behavior.
- Timestamps and message action bars.
- Search in conversation.

#### Coding-agent status and orchestration

- Agent card: active model, capabilities, endpoint, workspace, and permissions.
- Agent status: current phase, elapsed time, connection state, and cancellation affordance.
- Agent plan: ordered checklist with pending, active, completed, blocked, and failed steps.
- Activity graph: historical run activity.
- Background runs: tasks continuing while the user works elsewhere.
- Handoff: delegation between agents or sessions.
- Checkpoints: restore points with descriptions and affected files.
- Connection state: disconnected, reconnecting, resumed, and failed-to-resume states.
- Context display and context breakdown: prompt, files, tool results, and remaining context.
- Cost meter: token/model cost where the provider exposes it.

#### Coding output

- Code diff with unified additions/removals and file-level summaries.
- Code runner with command, live output, exit code, duration, and rerun.
- File tree with changed-file badges.
- File and image message parts.
- Artifact card for generated files, patches, reports, and downloads.
- Canvas for a full-width document or code review mode.
- Diagram, chart, data table, and image gallery renderers for tool-produced artifacts.
- Link preview and document reference for external sources.
- Confidence, inline citation, and source provenance where the agent uses research tools.

#### Human-in-the-loop controls

- Approval card for filesystem writes, command execution, network access, package installation, git operations, publishing, and destructive actions.
- Elicitation form for missing tool parameters.
- Guardrail notice for a refusal or blocked policy action.
- Feedback dialog for response/tool quality feedback.
- Error state with retry, not an unhandled exception or blank panel.

#### Composer extensions

- Attachments with previews, progress, removal, and per-file validation.
- Mentions for files, symbols, agents, and tools.
- Slash commands.
- Composer trigger popover.
- Models selector.
- Context indicator.
- Dictation as an optional capability.
- Follow-up suggestions.
- Command palette for application actions.

assistant-ui’s element catalog explicitly includes these patterns, including agent plans, approvals, artifacts, attachments, code diffs, code runners, context views, file trees, job progress, tool UI, and Markdown. [1]

### 5.3 RIGA-specific component composition

The public package should expose a small number of stable components and hooks:

```tsx
<RigaRuntimeProvider runtime={runtime}>
  <RigaShell
    sidebar={<RigaThreadList />}
    header={<RigaAgentCard />}
    inspector={<RigaContextPanel />}
  >
    <RigaThread />
  </RigaShell>
</RigaRuntimeProvider>
```

Recommended exports:

```ts
export {
  RigaRuntimeProvider,
  RigaThread,
  RigaComposer,
  RigaThreadList,
  RigaAgentCard,
  RigaAgentStatus,
  RigaPlan,
  RigaApprovalCard,
  RigaToolCall,
  RigaCodeDiff,
  RigaCodeRunner,
  RigaFileTree,
  RigaArtifactCard,
  RigaCheckpointList,
  RigaContextDisplay,
  RigaShell,
  useRigaRuntime,
  useRigaSession,
  useRigaWorkspace,
} from "@riga/assistant-ui";
```

Components should be composable. A host may use the full `RigaThread`, or use `ThreadPrimitive`, `ComposerPrimitive`, and `MessagePrimitive` directly for a custom layout. assistant-ui documents this as the path for materially different layouts. [3]

### 5.4 Tool UI convention

Tools executed by the kernel are represented as external tools in the React layer. assistant-ui’s `externalTool()` pattern is appropriate when execution happens in another system and the client only renders the live call, arguments, result, and status. [4]

RIGA tool renderers must be registered by stable tool name:

```ts
const rigaTools = {
  read_file: externalTool({ render: ReadFileToolCard }),
  write_file: externalTool({ render: WriteFileApprovalCard }),
  run_command: externalTool({ render: CommandRunnerToolCard }),
  apply_patch: externalTool({ render: DiffApprovalCard }),
  search_files: externalTool({ render: SearchResultsToolCard }),
  create_artifact: externalTool({ render: ArtifactToolCard }),
};
```

The browser must never infer that a tool succeeded from a button click. It sends an approval or cancellation command to the kernel and renders the result returned by the kernel.

## 6. Testing and quality requirements

RIGA must be developed with automated testing as a product requirement, not as a release-time activity. The target is high effective coverage of the kernel, adapters, transports, and user-critical React flows. Coverage percentages are release gates, but they are not the only quality signal: mutation testing, contract tests, deterministic fixtures, failure-path tests, and live-provider smoke tests are also required.

### 6.1 Test layers

Use the following test pyramid:

1. **Pure unit tests** — fast tests of state transitions, policy decisions, serialization, event reducers, path safety, model configuration, and UI utilities. Rust unit tests may test private functions inside `#[cfg(test)]` modules. React unit tests use Vitest and Testing Library.
2. **Component tests** — render assistant-ui/RIGA components against a deterministic mock runtime. Cover loading, streaming, approval, cancellation, error, empty, reconnect, and resume states.
3. **Kernel integration tests** — exercise the public `riga-kernel` API with fake models, fake tools, a temporary workspace, and a temporary session store. These verify multiple modules together without network cost.
4. **Transport contract tests** — send identical requests through Tauri, HTTP, CLI, and the in-process dispatcher. Assert equivalent JSON results, event sequences, error codes, and cancellation behavior.
5. **End-to-end tests** — run the desktop/web shell against a deterministic local kernel and verify a user journey from prompt to tool approval, diff, artifact, and resumed session.
6. **Live-provider tests** — opt-in tests using `OPENCODE_API_KEY`; never required for ordinary pull requests and never run with a secret on untrusted fork workflows.
7. **Non-functional tests** — coverage, mutation testing, race/concurrency checks, fuzzing of wire inputs, dependency auditing, accessibility checks, and package-boundary checks.

Rust’s official testing model separates fast in-module unit tests from external integration tests that exercise only the public API. RIGA should follow that separation and place crate-level integration tests in each crate’s `tests/` directory. [7]

### 6.2 Coverage targets

Set initial minimum line coverage gates as follows:

| Area | Minimum line coverage | Additional requirement |
|---|---:|---|
| `riga-kernel` domain/state/policy | 90% | 100% of public error codes and state transitions exercised |
| `riga-kernel` persistence/events | 85% | restart, replay, corruption, and migration cases |
| transport adapters | 85% | parity tests for every command and error mapping |
| server authentication/policy | 90% | deny-by-default and tenant-isolation tests |
| `riga-cli` | 80% | stdout/stderr and exit-code contract |
| Tauri commands | 80% | serialization, channel, cancellation, and initialization failures |
| `@riga/assistant-ui` runtime/protocol/hooks | 90% | all runtime state transitions and reconnect paths |
| critical React components | 85% | Testing Library interaction tests and accessibility assertions |
| overall frontend | 80% | no coverage reduction without an approved exception |

Exclude generated code, build output, third-party code, visual-only CSS, and test fixtures from percentage calculations, but do not exclude error handling or safety policy code merely because it is difficult to execute.

### 6.3 Definition of a tested feature

A feature is not complete until it has:

- a deterministic unit test for its normal path;
- tests for invalid input and authorization failure;
- cancellation/timeout coverage where asynchronous;
- persistence/reload coverage where stateful;
- transport parity coverage where exposed over a boundary;
- UI states for loading, ready, error, empty, and approval where applicable;
- a regression test for every production bug;
- a coverage contribution visible in CI.

### 6.4 Secret and live-provider policy

Never commit `OPENCODE_API_KEY`, a provider token, `.env` files containing secrets, copied Authorization headers, or live request/response logs containing credentials. Add `.env*` to `.gitignore` except for a checked-in `.env.example` containing placeholders only. Scan commits and CI artifacts for secret patterns.

Live provider tests must:

- be explicitly opt-in;
- read the key only from the process environment or CI secret store;
- pass the key to the test process as an environment variable, never as a command-line argument;
- use a small fixed prompt and a configured low-cost model;
- use strict timeout, retry, and response-size limits;
- redact request headers and response content in logs;
- avoid writing prompts or outputs to committed fixtures;
- use a dedicated test workspace with no user files;
- be skipped with a clear message when the key is absent;
- run only on trusted branches/environments in CI.

The user must provide the secret separately when a live test run is requested. The expected local setup is:

```bash
export OPENCODE_API_KEY='provided-out-of-band'
export RIGA_LIVE_LLM=1
cargo test -p riga-kernel --test opencode_go_live -- --nocapture
```

RIGA must not ask the user to paste the key into source files, chat logs, issue comments, or commit messages. GitHub Actions should expose it through an environment-level secret only on a manually approved or protected workflow. GitHub documents that secrets are not passed to fork-triggered workflows and recommends environment variables rather than command-line arguments; follow those rules. [9]

### 6.5 Live tests are not golden tests

Live model output is nondeterministic and must never be asserted by exact prose. A live test may assert:

- HTTP/auth succeeds;
- the response has a valid provider/model identifier;
- the response contains non-empty text;
- streaming emits ordered deltas and a terminal event;
- the request uses the configured session header;
- tool-capable smoke prompts produce a structurally valid tool call only when the selected model advertises tool support.

All semantic kernel tests use fake model adapters and committed deterministic fixtures. Live tests validate provider compatibility, not business correctness.

## 7. Kernel requirements

### 6.0 OpenCode Go and OpenAI-compatible model integration

Rig’s official OpenAI provider supports a custom `base_url`, which is the intended integration mechanism for OpenAI-compatible providers. [8] RIGA should therefore implement an `OpenAiCompatibleConfig` rather than an OpenCode-specific model type:

```rust
pub struct OpenAiCompatibleConfig {
    pub base_url: Url,
    pub api_key_env: String,
    pub model: String,
    pub session_id: String,
    pub user_agent: String,
}
```

For OpenCode Go, the default configuration is:

```text
base URL: https://opencode.ai/zen/go/v1
credential: OPENCODE_API_KEY
model: a current bare Go model ID, discovered/configured at runtime
session header: x-opencode-session: <stable RIGA session ID>
user agent: riga/<version>
```

OpenCode’s current Go documentation lists different endpoint families for different models: many models use `/chat/completions`, some use `/responses`, and some use an Anthropic-compatible `/messages` endpoint. [10] RIGA must not assume that every catalog model accepts Chat Completions. The provider registry should store the protocol family returned by the model catalog or by explicit configuration and select the matching Rig/provider adapter.

Do not hardcode an example model such as `big-pickle` as a required test dependency. Model catalogs and IDs can change. Resolve a model in this order:

1. explicit `RIGA_OPENCODE_MODEL`;
2. a checked-in test configuration containing a model known to be supported at the time of the release;
3. a live catalog lookup from the configured `/models` endpoint, if exposed;
4. a clear skip/error stating that no compatible model is configured.

The OpenCode Go docs also ask compatible clients to send a stable `x-opencode-session` header and identify themselves with their own user agent. [10] The RIGA adapter must preserve these headers. If the pinned Rig OpenAI builder does not expose custom headers, isolate a small HTTP-client/provider adapter or contribute the missing header hook upstream; do not silently omit the session identifier.

The live provider integration must be behind a feature such as `live-opencode` or a separately selected test target. Normal builds and tests must not require the secret, network access, or a provider account.

### 6.1 Public kernel responsibilities

`riga-kernel` must provide:

- kernel construction from `RigaConfig`;
- model/provider registration;
- tool registration and capability policy;
- session creation, listing, loading, archiving, and deletion;
- task start, stream, cancel, pause, resume, retry, and fork;
- message history and branch management;
- approval request and approval result handling;
- checkpoint creation and restore;
- workspace root and path-policy enforcement;
- event journal and resumable event cursor;
- artifact metadata and file references;
- structured error codes;
- health and capabilities inspection;
- deterministic test mode with fake model/tool implementations.

### 6.2 Proposed public API

```rust
pub struct RigaKernel { /* private state */ }

impl RigaKernel {
    pub async fn new(config: RigaConfig) -> Result<Self, RigaError>;
    pub async fn create_session(&self, req: CreateSessionRequest)
        -> Result<Session, RigaError>;
    pub async fn submit(&self, req: SubmitRequest)
        -> Result<RunAccepted, RigaError>;
    pub async fn stream(&self, req: StreamRequest)
        -> Result<impl Stream<Item = Result<RigaEvent, RigaError>>, RigaError>;
    pub async fn command(&self, req: KernelCommand)
        -> Result<KernelResponse, RigaError>;
    pub async fn approve(&self, req: ApprovalRequest)
        -> Result<ApprovalResult, RigaError>;
    pub async fn cancel(&self, req: CancelRequest)
        -> Result<(), RigaError>;
    pub async fn resume(&self, req: ResumeRequest)
        -> Result<RunAccepted, RigaError>;
}
```

The concrete Rig agent/runner and provider types remain implementation details unless a host explicitly needs an advanced integration. The public API should return stable domain types, not upstream internal types that would make semver maintenance difficult.

### 6.3 Event model

Every event includes `protocol_version`, `event_id`, `session_id`, `run_id`, `sequence`, and `timestamp`.

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct RigaEventEnvelope {
    pub protocol_version: u16,
    pub event_id: String,
    pub session_id: String,
    pub run_id: String,
    pub sequence: u64,
    pub timestamp: String,
    pub event: RigaEvent,
}

pub enum RigaEvent {
    RunStarted { prompt_message_id: String },
    TextDelta { delta: String },
    ReasoningDelta { delta: String },
    ToolCallStarted { call: ToolCall },
    ToolCallUpdate { call_id: String, patch: Value },
    ApprovalRequested { request: ApprovalRequest },
    ToolResult { result: ToolResult },
    FileChanged { change: FileChange },
    CommandOutput { output: CommandOutput },
    PlanUpdated { plan: AgentPlan },
    ArtifactCreated { artifact: Artifact },
    CheckpointCreated { checkpoint: Checkpoint },
    RunPaused,
    RunCompleted { result: RunResult },
    RunFailed { error: RigaErrorWire },
    RunCancelled,
}
```

The event journal makes reconnect and replay possible. A Tauri frontend may consume a live channel; a web client may reconnect with `after_sequence`; a test can replay a saved event fixture.

### 6.4 Bus ownership

The Tauri host creates the kernel during `setup()`, gives it the application data directory and configured model/tool registry, and stores an `Arc<RigaKernel>` in managed state. The kernel owns its bus driver and background tasks. Tauri does not recreate an agent for every IPC event.

The exact `rig-agent` bus API and version must be pinned and verified during implementation. The initial design may use the host-owned-bus pattern from the attached proposal, but the coding agent must compile a minimal proof before committing to an API shape.

### 6.5 Safety and permissions

The kernel enforces:

- workspace-root containment for all file operations;
- symlink escape checks;
- command allowlist/denylist and working-directory policy;
- environment-variable filtering;
- network permission scopes;
- package-install approval;
- git push and release approval;
- maximum output size and truncation metadata;
- timeout and cancellation propagation;
- audit events for every privileged tool call.

## 8. Tauri application requirements

The `riga` Tauri application is a thin adapter:

- register `submit`, `stream`, `cancel`, `approve`, `resume`, `list_sessions`, and `read_artifact` commands;
- forward serializable requests to `riga-kernel`;
- stream events through Tauri v2 `Channel` or a documented event channel;
- expose app data paths and OS integration only through Tauri;
- keep window/menu/tray/updater logic out of the kernel;
- use capabilities to restrict filesystem, shell, notification, and updater permissions;
- show an actionable error state if the kernel fails to initialize;
- never panic on a user prompt, malformed event, cancelled run, missing model, or stale session.

## 8.1 Mandatory sandbox and Tauri security requirements

The product plan must distinguish a shell call from a sandbox. `bash -lc ...` is only child-process execution. Tauri capabilities constrain WebView-to-Tauri IPC exposure; they do not constrain Rust application code, plugins, or a child process launched by Rust. Tauri’s shell plugin is a scoped process API, not an OS sandbox. [19] [20] [21]

Tauri’s Isolation Pattern is an IPC protection mechanism: a small isolated iframe intercepts and encrypts frontend IPC messages before they reach Tauri Core. It is valuable against compromised frontend dependencies but does not isolate filesystem, shell, network, or process execution. [23]

RIGA must implement the following mandatory execution architecture:

```text
assistant-ui → Tauri typed command → riga-kernel policy/approval
  → execution supervisor → restricted or OS-sandboxed worker → child process
```

### Mandatory execution profiles

- **Restricted local:** sanitized environment, workspace/path checks, timeout, output limits, resource limits, process-tree cleanup. This must be labelled restricted, not sandboxed.
- **OS sandbox:** default for model-generated commands; Linux namespaces, read-only runtime, explicit workspace mount, Landlock where available, seccomp, cgroups, and disabled network by default. A `bubblewrap`-style launcher may be used, but RIGA must generate and audit its configuration.
- **Disposable worker:** required for untrusted repositories, install scripts, arbitrary generated binaries, and hosted/network-enabled execution. Use a temporary container or microVM with no host credentials and explicit artifact export.

macOS and Windows must not be described as having Linux namespace isolation. They require platform-specific sandbox facilities or a disposable VM/container. If only restricted execution is available, high-risk capabilities must be disabled and the mode must be visible to the user.

### Mandatory supervisor behavior

The supervisor must own:

- canonical workspace and symlink/junction containment checks;
- clean allowlisted environment with no inherited provider, SSH, cloud, or package credentials;
- explicit stdin/stdout/stderr and bounded output;
- wall-clock, CPU, memory, process-count, and optional disk limits;
- process-group/descendant termination on timeout or cancellation;
- network mode and allowlist;
- audit events and execution IDs;
- secret redaction;
- controlled artifact export.

The React frontend must not directly invoke the Tauri shell plugin for agent commands. Every privileged action requires kernel-side authorization; a UI approval or Rig hook alone is insufficient.

### Mandatory Tauri v2 requirements

The implementation must use Tauri v2 precisely:

- separate least-privilege capability files under `src-tauri/capabilities/`;
- avoid capability overlap that unintentionally merges window privileges;
- expose only required commands through the invoke handler and command manifest where supported;
- use typed serializable command DTOs and stable serialized error codes;
- use `Channel<T>` for ordered, typed high-volume run streaming;
- reserve ordinary events for small notifications, not durable run state;
- keep filesystem, shell, HTTP, updater, process, notification, and window permissions separate;
- use explicit shell executable/argument scopes if the shell plugin is enabled;
- keep remote capabilities disabled unless explicitly threat-modelled and origin-allowlisted;
- enable restrictive CSP and avoid remote scripts/CDN dependencies;
- evaluate Tauri’s Isolation Pattern for the frontend dependency threat model;
- test capabilities, scopes, CSP, command exposure, channel ordering, and error serialization in CI.

Tauri scopes are not passive declarations: application command implementations must enforce them and must be audited for bypasses. [22]

### Sandbox acceptance gates

The design is not ready for implementation until tests prove that:

- unlisted privileged commands cannot be invoked from the frontend;
- a window cannot accidentally inherit another window’s capabilities;
- paths, symlinks, junctions, and mount escapes are rejected;
- worker environments contain no host credentials;
- timeout/cancellation kills process descendants;
- network-disabled mode has no network access;
- output and resource limits are enforced;
- disposable workers retain only explicitly exported artifacts;
- the UI reports restricted, OS-sandboxed, or disposable execution mode;
- Tauri IPC errors map to stable RIGA error codes.

## 9. Create.xyz integration

The official Create product documentation refers to **Create.xyz**, not a Rust package publishing service. It supports bringing an own REST API through Swagger/OpenAPI documentation, exporting code, and publishing the generated app. [6]

Therefore the plan is:

1. Publish `riga-kernel` as a Rust crate and source package through GitHub/crates.io workflows.
2. Publish `@riga/assistant-ui` to npm/GitHub Packages.
3. Publish `riga-server` as a deployable HTTP adapter or container.
4. Publish an OpenAPI document for `riga-server`.
5. Import that OpenAPI document into Create.xyz to generate a RIGA client surface.
6. Keep API keys and agent authority on the server. Never put provider secrets in Create-generated browser code.

Create-generated clients should receive only scoped, authenticated REST operations such as session creation, message submission, event streaming, artifact listing, and approval submission. A public web deployment must not expose unrestricted local filesystem or shell tools.

## Manus automation contract

The design is intended to be implemented by an unattended Manus coding agent. All ordinary phases must be runnable with non-interactive Rust/Node commands, deterministic fake models/tools, Vitest/Testing Library, headless Playwright UI automation, Tauri command/channel contract tests, and local service processes. No human click-through is part of the normal acceptance gate.

The only expected user-secret surface is the opt-in OpenAI-compatible live test using `OPENCODE_API_KEY`, supplied out-of-band or through a protected CI environment. The key must never enter source files, chat, logs, fixtures, screenshots, or artifacts. Code signing, GPU/CUDA, container/microVM, and live-provider workflows must be automated GitHub Actions jobs using protected secrets.

Because a Manus sandbox may not expose privileged namespaces, Docker, a VM, or a display server, the implementation must provide a portable restricted worker and deterministic fake worker for ordinary tests. Those tests prove policy and application behavior, not OS isolation. The OS-sandbox phase must feature-detect its backend, run conformance tests when available, and report `OS_BACKEND_UNAVAILABLE` rather than falsely passing when it is not available.

The authoritative UI gate is a headless Playwright suite against a fake kernel, covering streaming, tool calls, approvals, diffs, artifacts, cancellation, reconnect/replay, errors, accessibility, and no-duplicate-event behavior. Native Tauri behavior is covered by Rust command/channel contract tests and an optional automated desktop smoke job; it must not require a person to operate the app.

## Implementation sequencing

The detailed build order is defined in `RIGA-BUILD-PLAN.md`, Section 12. Implementation is gated through ten milestones: repository/toolchain lock; Rig compatibility proof; kernel persistence/replay; Tauri transport; assistant-ui parity; coding tools with restricted execution; OS/disposable isolation; HTTP/OpenAPI/Create.xyz; OpenCode Go live compatibility; and hardening/release.

The coding agent must finish one milestone before starting the next. Each milestone requires a written phase report containing the commands run, test results, artifacts, known limitations, and an explicit `PASS` or `BLOCKED` status. A skipped security test, unverified Rig/Tauri API, or platform-specific isolation gap is a blocking condition—not a warning.

## 10. Acceptance criteria for the design phase

The implementation is ready to start when:

- the package and crate names are consistently branded RIGA;
- the public wire contract is versioned and documented;
- the kernel owns product-level agent orchestration, tool policy, and security while delegating model/tool execution to Rig;
- the Tauri adapter contains no domain logic;
- the React package has a runtime-first integration plan;
- the required assistant-ui element coverage is mapped to components;
- tool execution and approval authority are explicitly server/kernel-side;
- the event envelope supports replay and reconnection;
- GitHub Actions release ownership is defined in `RIGA-BUILD-PLAN.md`;
- the Create.xyz integration is based on OpenAPI/REST, not an assumption that Create publishes Rust crates.

## References

[1]: https://www.assistant-ui.com/elements "assistant-ui Elements catalog"
[2]: https://www.assistant-ui.com/docs/runtimes/custom/overview "assistant-ui Custom Runtime overview"
[3]: https://www.assistant-ui.com/elements/thread "assistant-ui Thread element and primitive composition"
[4]: https://www.assistant-ui.com/docs/tools/defining-tools "assistant-ui Defining Tools and external tool rendering"
[5]: https://raw.githubusercontent.com/haymant/fina-builder/main/FEATURES.md "Fina Builder feature and architecture document"
[6]: https://create.xyz/how-it-works "Create.xyz product and REST API integration overview"
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
