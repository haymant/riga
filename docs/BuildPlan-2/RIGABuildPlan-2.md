# RIGA Graph-Native Runtime Evolution

**Implementation handoff for a coding agent.**

> **Reviewed** against `main` @ `d971ca9`. Statements about the *current*
> implementation that did not match the code are marked inline with
> **Review:** and collected in
> [Reviewer notes — corrections vs. current code](#reviewer-notes--corrections-vs-current-code).
> Open questions that were resolved during review are marked **Decision:**.

## Executive Summary

This document supersedes earlier graph-engineering proposals.

After reviewing the actual RIGA architecture, the key conclusion is:

RIGA already possesses the most important primitive that many agent systems lack:

an explicit subagent runtime with typed worker roles, live task lifecycle events,
parent-child task relationships, ~~concurrent workers,~~ approval gates, durable
task records, and AssistantUI integration.

> **Review:** There is **no concurrency** today. `dispatch_subagent`
> (`crates/riga-server/src/ws.rs:2307`) runs each child inline and `.await`s it,
> so subagents are strictly sequential. "Parent-child task relationships" are also
> nominal: `TaskRecord.parent_id` is set to a synthetic `depth-{depth}` string
> (`ws.rs:2361`), not a real parent task id, and the durable `TaskTree` type
> (`crates/riga-kernel/src/task.rs:52`) is **unused at runtime** — only its own
> unit tests construct it. Durable task records do exist, but only as
> `TaskStarted`/`TaskCompleted` frames in the per-run journal.

Therefore:

**Do not build a new graph system alongside RIGA. Instead: promote existing RIGA
task infrastructure into a graph-native orchestration runtime.**

## Core Principle

Current RIGA:

```text
Root Agent
    │
    ├── @explore
    ├── @plan
    ├── @build
    └── @review
```

Current task hierarchy:

```text
Tree
```

> **Review:** It is a flat list of `TaskRecord`s in the UI
> (`AgentTaskView { id, agent, description, state }`,
> `packages/assistant-ui/src/AssistantUI.tsx:189`), not a navigable tree. The
> `parent_id` field is not surfaced and is not a real id.

Target:

```text
Execution Graph
        +
Evidence Graph
        +
Knowledge Graph
```

built on top of:

```text
TaskRecord
TaskStarted
TaskCompleted
TaskStatus
Task dispatch
Subagents
```

> **Review:** `TaskStatus` is declared (`crates/riga-kernel/src/events.rs:36`) but
> the server never emits it — only `TaskStarted` and `TaskCompleted` are sent
> (`ws.rs:2369`, `ws.rs:2424`).
>
> **Decision:** wire `TaskStatus` from `dispatch_subagent` at the
> `Pending → Running → WaitingForApproval → Completed/Failed` transitions. It is
> cheap, it makes the Subagents card a live ledger instead of a start/finish log,
> and the variant already exists.

rather than replacing them.

## What Makes RIGA Different

Most coding agents expose:

```text
Planner
↓
Tool Calls
↓
Tool Calls
↓
Done
```

RIGA already exposes:

```text
Planner
↓
Subagents
↓
Task Lifecycle
↓
Sequential Workers
↓
Durable Records
```

> **Review:** "Concurrent Workers" corrected to "Sequential Workers" (see above).

That is a much stronger foundation.

The strategic goal is not:

```text
better prompting
```

but:

```text
better runtime structure
```

especially for:

- Phi-4
- Qwen3 4B
- Qwen3 8B
- other small local models

which struggle with long-running reasoning and progress tracking.

## Existing Agent Profiles

The graph architecture must embrace existing profiles. Do not create new
planner/executor abstractions. Leverage what already exists.

Definitions: `crates/riga-server/src/catalog.rs:306-382`. Tool gating:
`allowed_tools_for` (`ws.rs:2469`).

### `@explore` (aliases `scout`, `explorer`)

Purpose: repository reconnaissance, context compression, architecture discovery.

Output: Summary, Answer, Files Retrieved, Key Code, Architecture, Start Here.

Current strengths: cheap model, read-only, safe, fast.

Future role: **Evidence Producer**.

Responsibilities: discover files, discover symbols, discover architecture,
produce evidence nodes.

NOT: create plans, modify graph structure, make implementation decisions.

### `@plan` (alias `planner`)

Current purpose: requirements → implementation plan.

Future purpose: requirements → **Graph Constructor** → execution plan.

Output remains: Goal, Plan, Files to Modify, New Files, Risks / Assumptions.

The planner additionally becomes responsible for: task decomposition, dependency
creation, parallelization opportunities, worker assignment, success-criteria
creation, evidence requirements.

Planner becomes: **Graph Architect**.

> **Review (design constraint):** `plan` is read-only, so `allowed_tools_for`
> grants it only `update_plan`, `update_todos`, `read`, `glob`, `grep`, `skill`
> — it cannot call `task` to dispatch or create tasks.
>
> **Decision:** the planner *publishes* the graph through a new `update_graph`
> run-state tool (a sibling of `update_plan`/`update_todos`), which is safe to
> grant to read-only profiles because it only records run state. The planner
> never dispatches; the runtime (Phase 1) or the scheduler (Phase 2) does. See
> [Graph payload contract](#graph-payload-contract-decided).

### `@build` (aliases `executor`, `worker`)

Current role: implementation worker.

Future role: **Artifact Producer**.

Produces: code changes, validation results, artifacts.

Should never: own scheduling, create arbitrary graph structure.

Planner owns graph. Build executes graph.

> **Review:** `build` is the only mutating profile and the only one granted
> `task`, so it can already nest one level of dispatch (`MAX_TASK_DEPTH = 2`,
> `MAX_TASK_FANOUT = 4`, `crates/riga-kernel/src/task.rs:47`).

### `@review` (alias `reviewer`)

Current role: independent reviewer.

Future role: **Validation Producer**.

Produces: findings, risk assessments, validation evidence, confidence scores.

Should produce evidence-backed findings, not freeform reviews only.

## Revised Runtime Model

Current:

```text
User
 ↓
Agent
 ↓
Subagents
```

Future:

```text
User
 ↓
Planner
 ↓
Graph
 ↓
Scheduler
 ↓
Explore / Build / Review
 ↓
Evidence
 ↓
Knowledge
```

## Highest ROI Problem to Solve

### Current pain point

Today task progress typically behaves like:

```text
Task 1
Task 2
Task 3

UI:
0/3
...
0/3
...
0/3
...
3/3
```

at the very end.

This is a common failure mode. The agent internally knows it is progressing; the
runtime does not. AssistantUI only receives completion after large chunks of work.
Result: the user sees no useful progress.

> **Review:** Two distinct progress mechanisms already exist and should not be
> conflated:
> - `PlanUpdated` / `TodoUpdated` drive the live `AgentPlanCard` and
>   `AgentTodoList` (`AssistantUI.tsx:707`). These **do** update mid-run, but
>   only when the model actually calls `update_plan` / `update_todos`.
> - The **Subagents** card (`AgentTaskList`, `AssistantUI.tsx:925`) shows one row
>   per `TaskStarted`/`TaskCompleted` with `running | done | failed` — no
>   per-task progress.
>
> The "0/3 … 3/3 at the end" symptom is mostly the Subagents card plus a model
> that under-uses `update_plan`. The fix is both runtime (emit progress) and
> prompt (call the tools early).

### Desired behaviour

Planner creates: Investigate Runtime, Investigate UI, Review Security, Generate
Plan.

Immediately: `0/4`.

Explore Runtime completes: `1/4`.

Explore UI completes: `2/4`.

Review starts: `2/4 running review`.

Final Plan: `4/4` — without waiting for the entire run.

The runtime becomes **task-driven** instead of **response-driven**.

## Execution Graph

Extend existing `TaskRecord`. Current shape
(`crates/riga-kernel/src/task.rs:33`):

```rust
pub struct TaskRecord {
    pub id: String,                 // NOTE: String, not a TaskId newtype
    pub parent_id: Option<String>,
    pub agent: String,
    pub description: String,
    pub model: String,
    pub state: TaskState,           // Pending|Running|WaitingForApproval|Completed|Failed|Cancelled
    pub started_at: String,
    pub result: Option<String>,
}
```

> **Review:** The proposed struct below *replaces* `agent`/`model`/`started_at`/
> `result` and adds a `profile` field. Extend the existing record instead of
> redefining it, and keep the `TaskState` enum and the terminal-state invariant
> (`set_state` refuses to mutate a settled task, `task.rs:128`).

Future (extended, not replaced):

```rust
pub struct TaskRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent: String,
    pub description: String,
    pub model: String,
    pub state: TaskState,
    pub started_at: String,
    pub result: Option<String>,
    // new:
    pub progress: f32,
    pub confidence: Option<f32>,
    pub evidence_count: usize,
    pub knowledge_count: usize,
    pub dependencies: Vec<String>,
    pub blocked_by: Vec<String>,
}
```

### Why dependencies matter

Current tree:

```text
Root
├── Explore Runtime
├── Explore UI
└── Build
```

Build may begin too early. Desired:

```text
Build
  ▲
  │
Explore Runtime

Build
  ▲
  │
Explore UI
```

Build becomes runnable only when dependencies complete. The scheduler manages
this. The LLM does not.

## Graph Scheduler

Add `GraphScheduler`.

Responsibilities: Ready Queue, Blocked Queue, Running Queue, Completed Queue.

Rules: all dependencies satisfied → runnable; otherwise → blocked.

### Ownership (decided, in two phases)

Fully autonomous scheduling is the target, but it is a large behavioural change
to land at once, so the runtime takes ownership in two steps:

- **Phase 1 — the runtime owns readiness; the orchestrator issues the dispatch.**
  On `GraphUpdated`, the runtime computes the ready set. A `task dispatch` for a
  node whose dependencies are not complete is rejected/blocked, and
  `TaskRunnable` / `TaskBlocked` are emitted so both the model and the UI see the
  state. This delivers the Phase-1 ROI (visible progress, correct states) with
  low risk and keeps the LLM as the safety valve.
- **Phase 2 — the scheduler owns dispatch.** Once the graph contract and
  readiness rules are stable and tested, a server-side `GraphScheduler` runs
  ready nodes itself, with no LLM in the dispatch loop.

Committed scope for Phase 1 is the first bullet; Phase 2 is explicitly "after the
graph contract is stable".

## Planner-Generated Graphs

Planner should generate machine-readable graph plans. Alongside normal output
(Goal, Plan, Files to Modify), add a hidden graph payload:

```json
{
  "tasks": [
    { "id": "runtime", "profile": "explore" },
    { "id": "ui", "profile": "explore" },
    { "id": "review", "profile": "review", "depends_on": ["runtime", "ui"] }
  ]
}
```

Scheduler interprets.

### Graph payload contract (decided)

The planner publishes the graph through a new `update_graph` run-state tool — a
sibling of `update_plan` / `update_todos` (`ws.rs:1391`). It deserializes into a
new `riga_kernel::task::Graph`, validates it, and emits `GraphUpdated`. Because it
only records run state, it is safe to grant to read-only profiles, so `@plan` can
publish a graph without a new capability class.

A fenced `json graph` block in the planner's result is accepted as a **fallback**
through the same validation path, for small local models that do not call tools
reliably. It is not the primary carrier.

Contract (extend `riga-kernel/src/task.rs`, next to `Plan` / `TodoList`):

```rust
pub struct Graph { pub title: String, pub nodes: Vec<GraphNode> }
pub struct GraphNode {
    pub id: String,               // unique
    pub profile: String,          // resolves via catalog::find_agent_profile
    pub description: String,
    pub prompt: String,
    pub depends_on: Vec<String>,  // must reference existing ids
}
```

Validation on receipt, rejected with an actionable error like `update_plan`:
unique ids; known profiles; `depends_on` targets exist; acyclic; within
`MAX_TASK_FANOUT` / `MAX_TASK_DEPTH`; profile gates respected.

The graph is **run state**, like `Plan`: held by the run, emitted as
`GraphUpdated`, and journaled (see [Event Bus Expansion](#event-bus-expansion)).
It is not stored in the database — run journals are files.

## Evidence Graph

Most important feature after the scheduler.

### Problem

Current output:

```text
The task system uses depth limit 2.
```

User must trust the agent.

### Desired

```text
Claim:      Depth limit is 2.
Evidence:   task.rs:47 / MAX_TASK_DEPTH
Confidence: 0.98
```

> **Review:** The cited symbol was wrong for this repo. The limit is
> `pub const MAX_TASK_DEPTH: usize = 2;` (`crates/riga-kernel/src/task.rs:47`),
> enforced in `TaskTree::spawn` (`task.rs:95`). There is no
> `TaskTree::validate_depth`.

### Model

```rust
pub struct EvidenceNode {
    id: EvidenceId,
    task_id: TaskId,
    source_type: SourceType,
    source_ref: String,
    snippet: String,
    confidence: f32,
}
```

Sources: File, Read Result, Tool Output, Command Output, Test Output,
Documentation.

## Knowledge Graph

Purpose: what has the agent learned? (not: what task is running?)

Example — Knowledge: `TaskRecord` contains task lifecycle and parent links.
Future runs reuse `TaskRecord` without rescanning.

> **Review (important):** There are **no knowledge-oriented UI primitives**
> today. There is no renderer abstraction, no knowledge UI, and no evidence UI
> anywhere in `packages/assistant-ui/src` (grep for `renderer|knowledge|evidence`
> returns nothing). The existing primitives are: transcript + `Markdown`, the
> tool timeline / tool cards, the reasoning block, `AgentPlanCard`,
> `AgentTodoList`, and `AgentTaskList`.
>
> **Decision:** build knowledge/evidence rendering as new cards in the
> [Run Deck](#the-run-deck-first-class-and-collapsible), reusing the existing
> `agent-*` card pattern. Do not invent a renderer plugin system.

## Loop Detection

### Current failure

```text
grep
grep
grep
grep
```

Small model stalls.

### Add

`ToolSignature` = tool + args hash + result hash.

Emit `LoopDetected` when repeated.

Provide: "You appear stuck. Summarize findings. Choose a new strategy."

> **Review:** Partial detection already exists in the local loop:
> `MAX_REPEATED_TOOL_CALLS = 2` (`ws.rs:2512`),
> `MAX_CONSECUTIVE_TOOL_FAILURES = 3` (`ws.rs:2502`),
> `LOCAL_MAX_TOOL_CALLS = 16` (`ws.rs:2507`), and the `LOCAL_TOOL_CALL_RETRY`
> nudge (`ws.rs:2514`).
>
> **Decision:** extend the existing limits rather than adding a new mechanism:
> have them emit `LoopDetected` (with the nudge text) and apply the same checks
> to remote runs, which currently have none.

## Repository Intelligence Layer

Highest ROI for small models.

Before: `grep repository`. After: `find_symbol`, `find_callers`,
`find_references`, `find_tests`.

Backed by: tree-sitter, tantivy, sqlite.

> **Review:** New dependencies. Note `sqlx` (sqlite + postgres) is already in
> `crates/riga-server/Cargo.toml`; tree-sitter/tantivy would be new. The current
> tools are `read`/`glob`/`grep` (`catalog.rs`), so this is additive.

## AssistantUI Strategy

Critical: DO NOT rebuild features AssistantUI already offers. This work is an
**enhancement of the existing panels above the composer**, not a new surface.

### The Run Deck (first-class and collapsible)

Today the run's live state renders in `.agent-work`, pinned above the composer
(`styles.css:529`; `AssistantUI.tsx:707`), as sibling cards: `AgentPlanCard`,
`AgentTodoList`, and `AgentTaskList` (the "Subagents" card). Promote that region
to a **first-class, collapsible UI element**: the **Run Deck**.

- **Name.** `Run Deck` — the live deck of the current run: graph, plan, todos,
  subagents, and (later) evidence and knowledge. Component `RunDeck`, class
  `.run-deck`. It reads naturally in a header: `Run Deck · 2/4 · running review`.
- **Collapse/expand is mandatory, not optional.** The deck owns one summary
  header (progress `n/m`, the running node, ready/blocked counts) and collapses
  to just that header. Expanded, it stacks the existing cards; collapsed, it is a
  single line.
- **Mobile first.** The current task list / ledger can grow tall enough on a
  phone to push the chat off-screen. On narrow viewports the deck **defaults to
  collapsed**, keeps a compact header, and expands as an overlay above the
  transcript rather than permanently consuming vertical space. Persist the
  collapsed state per session.
- **Compose, don't duplicate.** The deck composes the cards that already exist;
  new cards (graph, evidence, knowledge) join the same stack rather than
  becoming separate widgets or pages.

Alternatives considered: `Workstream`, `Execution Panel`, `Graph Panel`. `Run
Deck` wins because it is short, ties to RIGA's run model (`run_id`,
`RunStarted`/`RunCompleted`), and "deck" implies a stack of collapsible cards,
which is exactly the collapse/expand requirement.

#### Switching graphs (one deck, two axes)

A single deck still has to let the user choose *which* graph to look at. There
are two axes, and both are handled inside the deck's header — no new panels.

1. **Graph kind** — the three graphs in this document: Execution, Evidence,
   Knowledge. A segmented control in the summary header selects the lens; kinds
   with no data are disabled.

   ```text
   Run Deck   [ Execution | Evidence | Knowledge ]   Run 3 ▾   2/4 · running review
   ```

2. **Graph scope** — which run, and which sub-graph inside it:
   - The deck **follows the active run** by default, so it stays live.
   - A **run picker** (`Run 3 ▾`) lists the runs in the session. Picking a past
     run loads its graph from that run's journal
     (`<data_root>/runs/run-<id>.json`) — **read-only** and marked historical,
     because the journal already holds `TaskStarted` / `TaskCompleted` /
     `GraphUpdated`.
   - **Drill-down:** clicking a subagent node opens *its* sub-graph; a breadcrumb
     pops back.

     ```text
     Session › Run 3 › review        ← click to pop back up
     ```

   - **Optional aggregate:** a `Session` entry merges every run into one graph
     (nodes = tasks across runs, edges = parent / `depends_on`). Not required
     for v1.

The deck's view state is `{ runId, kind, focusNode }`, persisted per session. On
mobile the deck is collapsed by default, so the selector row is hidden until
expanded; the collapsed header still carries the active state
(`Run Deck · Execution · Run 3 · 2/4 · running review`).

Naming: **graph lens** for the kind selector (the three kinds are lenses on the
same run), **run scope** for the picker.

### Agents UI

Current Subagents card already aligns with agent orchestration. Expand. Do not
replace.

Add: dependency status, progress percentages, blocked/runnable badges inside
agent views.

> **Review:** The card is `AgentTaskList` (`AssistantUI.tsx:918`), fed by
> `TaskStarted`/`TaskCompleted` handlers (`AssistantUI.tsx:420`, `:423`). It has
> `running | done | failed` and task-scoped tool runs only. Wiring `TaskStatus`
> (see Core Principle) is what turns it into a live ledger.

### Knowledge UI

The Knowledge Graph, Evidence Collections, Facts, and Findings render as cards in
the Run Deck, using the same `agent-*` card pattern, rather than as separate
pages or widgets.

### Tool Use UI

Current task-scoped tool rendering is already excellent. Enhance with: evidence
links, tool cost, loop-detection warnings — rather than redesigning.

### Renderer Strategy

"Renderers" here means React components selected by event/role inside the
transcript and the Run Deck — not an existing plugin system.

> **Review:** There is no renderer registry. There is nothing to extend; the
> components are new, and they should be small and selected the same way the
> current transcript items are.

## Graph Visualization

Render the execution graph inside the Run Deck as an interactive **XYFlow
panel** using [`@xyflow/react`](https://reactflow.dev). XYFlow ships zoom, pan,
and fit-to-view out of the box, so the graph is cheap to build and behaves well
on desktop and mobile without any bespoke canvas work.

- **Nodes** — one per `GraphNode` / `TaskRecord`, tinted by state
  (`pending | running | blocked | done | failed`) and badged ready/blocked.
- **Edges** — `depends_on` becomes a directed edge; satisfied edges are solid,
  blocked edges dashed, so readiness is visible at a glance.
- **Layout** — a small layered/topological layout (row = dependency level). No
  custom physics, no XYFlow auto-layout plugins.
- **Placement** — one card in the Run Deck, so it inherits collapse/expand and
  the summary header. It is not a separate page.
- **One panel, three lenses** — the same panel renders the selected kind
  (Execution / Evidence / Knowledge); the kind and run/scope selectors live in
  the deck header (see
  [Switching graphs](#switching-graphs-one-deck-two-axes)).
- **Fallback** — the collapsed deck and the pure-logic tests use a plain
  dependency list + badges, so correctness never depends on XYFlow rendering.

```text
Explore Runtime ─┐
                 ├─▶ Review ─▶ Build
Explore UI ──────┘
```

> **Review:** `@xyflow/react` is not currently a dependency. It is added in
> Phase 3 (see the phased plan). The graph **adapter** (graph → nodes, edges,
> topological rows, state → tint) is pure and unit-tested without the DOM, so
> the risky part is covered by vitest and the XYFlow boundary stays thin.

## Event Bus Expansion

Extend `RigaEventEnvelope` with:

```text
TaskDependencyAdded
TaskBlocked
TaskRunnable
EvidenceAdded
EvidenceLinked
KnowledgeCreated
KnowledgeLinked
LoopDetected
GraphUpdated
```

Everything else subscribes to events.

> **Review:** Add these as `RigaEvent` variants (`crates/riga-kernel/src/events.rs`).
> `RunChannel::emit` (`ws.rs:1619`) journals every non-ephemeral event, and
> `is_ephemeral` currently lists only `TextDelta` and `ToolOutputDelta`.
>
> **Decision:** `GraphUpdated`, `TaskDependencyAdded`, `TaskRunnable`,
> `TaskBlocked`, `EvidenceAdded`, `EvidenceLinked`, `KnowledgeCreated`,
> `KnowledgeLinked`, and `LoopDetected` are low-frequency and authoritative, so
> they are **journaled** (not ephemeral). While in here, also add `ReasoningDelta`
> to `is_ephemeral` — it is currently journaled, so every reasoning token
> rewrites the run file.

## Phased Plan (entry / exit / test evidence)

Every phase has explicit **entry criteria**, **exit criteria**, and the **test
evidence** that proves it. Runtime phases are proven by Rust tests; UI phases
target high **vitest** coverage. The vitest config currently scopes
`include` / `coverage.include` to `packages/assistant-ui/src/http/**` and
`.../tauri/**` (`vitest.config.ts`), so Phase 0 widens it before any new UI
lands.

### Phase 0 — UI test harness (enabler)

- **Entry:** repo green (`cargo test --workspace`, `npm test`).
- **Work:** add `jsdom` + `@testing-library/react`; widen `vitest.config.ts`
  `include` and `coverage.include` to the new UI modules; add a `RunDeck` smoke
  test; keep the existing `http` / `tauri` suites green.
- **Exit:** `npm test` runs the UI suites; the coverage thresholds
  (`lines/functions/statements ≥ 80`, `branches ≥ 75`) apply to the new modules,
  not just `http`/`tauri`.
- **Test evidence:** `npm test` lists the new files in the coverage report; a
  trivial `RunDeck` render test passes.

### Phase 1 — Graph payload + readiness (runtime)

- **Entry:** Phase 0 harness in place; the graph contract in this document
  frozen.
- **Work:** `update_graph` tool; `riga_kernel::task::Graph` + validation;
  `GraphUpdated`; runtime readiness; `TaskDependencyAdded` / `TaskRunnable` /
  `TaskBlocked`; reject a `task dispatch` whose dependencies are incomplete; wire
  `TaskStatus`; journal the new events and add `ReasoningDelta` to
  `is_ephemeral`.
- **Exit:** a planner-emitted graph validates; ready/blocked sets are correct; a
  blocked dispatch is rejected with an actionable error; the new events appear on
  the wire and in the run journal.
- **Test evidence (Rust):** `cargo test --workspace` with new unit tests for
  validation (duplicate id, unknown profile, missing dependency, cycle,
  depth/fanout overflow), readiness computation, blocked-dispatch rejection, and
  `TaskStatus` emission.

### Phase 2 — Run Deck shell (UI)

- **Entry:** Phase 1 events available.
- **Work:** promote `.agent-work` to `RunDeck` (`AssistantUI.tsx:707`,
  `styles.css:529`); summary header (`n/m`, running node, ready/blocked counts);
  collapse/expand; mobile default-collapsed with an overlay expansion; persist
  the collapsed state per session; the selector row — **graph lens**
  (`Execution | Evidence | Knowledge`) and **run scope** picker with sub-graph
  drill-down — backed by view state `{ runId, kind, focusNode }`.
- **Exit:** the deck renders from events, collapses to a single line, expands to
  the card stack, defaults collapsed under the mobile breakpoint, and switches
  kind / run / sub-graph from the header.
- **Test evidence (vitest):** rendering + interaction tests for `RunDeck` —
  summary counts, toggle, persisted state, mobile default, and selector
  switching (kind, run, drill-down/breadcrumb). High coverage of the deck module.

### Phase 3 — XYFlow graph panel

- **Entry:** Phase 2 shell + graph events.
- **Work:** `RunGraphPanel` on `@xyflow/react`; pure adapter (graph → nodes,
  edges, topological rows, state → tint); zoom/pan/fit; solid vs dashed edges for
  satisfied vs blocked; the panel re-renders when the header selector changes
  kind (Execution / Evidence / Knowledge) or scope (run / sub-graph); collapsed
  fallback = dependency list + badges.
- **Exit:** nodes/edges match the graph; zoom/pan/fit work; changing kind or
  scope swaps the panel content; historical runs render read-only; the fallback
  list is shown when collapsed.
- **Test evidence (vitest):** the pure adapter is unit-tested to high coverage
  (node/edge mapping, state tints, topological levels, blocked edges); a jsdom
  smoke test renders the panel behind a thin mock of the XYFlow boundary and
  asserts it re-renders on a selector change.

### Phase 4 — Evidence graph

- **Entry:** Phase 3.
- **Work:** `EvidenceNode`; `EvidenceAdded` / `EvidenceLinked`; evidence cards in
  the Run Deck; link a claim to its `source_ref` and confidence.
- **Exit:** a claim carries evidence with a source reference and a confidence
  value; the deck renders evidence cards.
- **Test evidence:** Rust tests for evidence emission and linking; vitest for the
  evidence card (render, empty state, confidence display).

### Phase 5 — Repository intelligence

- **Entry:** Phase 4.
- **Work:** `find_symbol` / `find_callers` / `find_references` / `find_tests`;
  index backend.
- **Exit:** symbol and reference queries return `file:line` evidence.
- **Test evidence:** Rust tests over a fixture repo; tool-schema tests.

### Phase 6 — Knowledge graph

- **Entry:** Phase 5.
- **Work:** `KnowledgeCreated` / `KnowledgeLinked`; cross-run reuse;
  checkpointing.
- **Exit:** knowledge persists across runs and is reused without rescanning.
- **Test evidence:** Rust tests for persistence and reuse; vitest for the
  knowledge card.

## Success Metric

The runtime should no longer behave like:

```text
Large Prompt
↓
Long Silence
↓
Final Answer
```

It should behave like:

```text
Planner
↓
Graph Created
↓
Tasks Running
↓
Evidence Produced
↓
Knowledge Accumulated
↓
Review Completed
↓
Answer Generated
```

with every state visible through the Run Deck and the existing transcript and
tool-use primitives.

The graph becomes the source of truth. The conversation becomes merely a view of
the graph.

---

## Reviewer notes — corrections vs. current code

Verified against `main` @ `d971ca9`.

| # | Claim in the document | Reality | Reference |
|---|---|---|---|
| 1 | "Concurrent workers" | Subagents are sequential; `dispatch_subagent` `.await`s each child inline | `ws.rs:2307-2418` |
| 2 | "Parent-child task relationships" / "Tree" | `parent_id` is a synthetic `depth-{depth}` string; `TaskTree` is unused at runtime (tests only); UI shows a flat list | `ws.rs:2361`, `riga-kernel/src/task.rs:52`, `AssistantUI.tsx:189` |
| 3 | `TaskStatus` is a live primitive | Declared but never emitted; only `TaskStarted`/`TaskCompleted` are sent | `events.rs:36`, `ws.rs:2369`, `ws.rs:2424` |
| 4 | "AssistantUI already provides knowledge-oriented UI primitives" | None exist | grep `renderer\|knowledge\|evidence` in `packages/assistant-ui/src` → empty |
| 5 | "AssistantUI already supports rich renderers" | No renderer registry; only transcript/role selection | `AssistantUI.tsx` |
| 6 | Proposed `TaskRecord` replaces fields | Extend the existing record; keep `agent`/`model`/`started_at`/`result` and `TaskState` | `riga-kernel/src/task.rs:33-44` |
| 7 | `TaskId` / `TaskTree::validate_depth` | Ids are `String`; the limit is `MAX_TASK_DEPTH = 2`, enforced in `TaskTree::spawn` | `riga-kernel/src/task.rs:47,95` |
| 8 | Planner constructs the graph | `plan` is read-only and cannot dispatch; the runtime materializes the graph | `ws.rs:2469-2474` |
| 9 | Loop detection is missing | Partial limits exist (`MAX_REPEATED_TOOL_CALLS`, `MAX_CONSECUTIVE_TOOL_FAILURES`, `LOCAL_MAX_TOOL_CALLS`) but emit no event and apply to local only | `ws.rs:2502-2514` |

Confirmed accurate: the four profiles and their outputs (`catalog.rs:306-382`),
`MAX_TASK_DEPTH = 2` / `MAX_TASK_FANOUT = 4`, approval events, `update_plan` /
`update_todos`, `PlanUpdated` / `TodoUpdated` driving live agent-plan/todo cards,
and durable per-run task frames via the journal.

### Decisions folded in this pass

1. **Graph carrier:** `update_graph` run-state tool → `Graph` type → `GraphUpdated`;
   fenced `json graph` block as a fallback. Validation rules listed above.
2. **Scheduler ownership:** Phase 1 runtime-owns-readiness / orchestrator-dispatches;
   Phase 2 scheduler-owns-dispatch.
3. **`TaskStatus`:** wire it at task state transitions.
4. **Ephemerality:** new graph/evidence/knowledge/loop events are journaled;
   add `ReasoningDelta` to the ephemeral set.
5. **Knowledge/renderer UI:** build new Run Deck cards on the `agent-*` pattern;
   no plugin registry.
6. **Graph panel:** render with XYFlow (`@xyflow/react`) in the Run Deck —
   zoom/pan/fit for free — with a plain dependency-list fallback.
7. **Loop detection:** extend the existing local limits and emit `LoopDetected`;
   apply to remote runs.
8. **AssistantUI area:** promote `.agent-work` to the **Run Deck**, a first-class,
   collapsible element above the composer.
9. **Graph switching:** one deck, a header selector row — a **graph lens**
   (Execution / Evidence / Knowledge) and a **run scope** picker with sub-graph
   drill-down; past runs load read-only from the journal.
