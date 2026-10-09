# BuildPlan-2 Phase 1 Report — Graph payload and readiness

## Status

**Complete.** The planner can publish a validated execution DAG through `update_graph`, and explicit graph-node dispatch observes dependency readiness.

## Implemented

- Added `riga_kernel::task::Graph` and `GraphNode`.
- Added structural validation for:
  - empty graphs;
  - duplicate node IDs;
  - missing dependency IDs;
  - self-dependencies and cycles;
  - dependency depth beyond `MAX_TASK_DEPTH`;
  - dependency fan-out beyond `MAX_TASK_FANOUT`.
- Added `Graph::ready_nodes` to calculate nodes whose dependencies are completed.
- Added wire events:
  - `GraphUpdated`;
  - `TaskDependencyAdded`;
  - `TaskRunnable`;
  - `TaskBlocked`.
- Added per-run graph state to the existing `RunEvidence` context rather than introducing a second orchestration runtime.
- Added `update_graph` to the server tool schema and to read-only profile capabilities, allowing `@plan` to publish a graph without gaining mutation permissions.
- Added `node_id` and `parent_task_id` dispatch fields.
- Explicit dispatches with `node_id` now reject incomplete dependencies with an actionable error and emit `TaskBlocked`.
- Graph node dispatches emit `TaskStarted`, `TaskStatus` running/terminal transitions, and `TaskCompleted`.
- Graph, dependency, runnable, and blocked events are durable; `ReasoningDelta` is now treated as ephemeral alongside text/tool-output deltas.
- Added orchestration prompt guidance for publishing and dispatching graph nodes.

## Test evidence

```text
cargo fmt --all -- --check
```

Passed.

```text
cargo test --workspace
```

Passed:

- `riga-kernel`: 32 tests
- `riga-server`: 91 tests
- `riga-shell`: 1 test
- All workspace doc tests

Focused coverage includes graph validation, readiness transitions, duplicate/missing/cyclic/deep/fan-out rejection, event serialization, update_graph event emission, blocked dispatch rejection, and durable-versus-ephemeral event classification.

```text
npm test
```

Passed:

- 5 test files
- 41 tests
- 89.15% statements
- 94.09% lines
- 86.71% functions
- 78.70% branches

```text
npm run check
```

Passed naming lint and root TypeScript validation.

## Scope boundary

Phase 1 intentionally implements **runtime-owned readiness with orchestrator-issued dispatch**. A server-side scheduler that automatically dispatches all ready nodes is Phase 2+ work after the graph contract is stable. Graph state is run state and is journaled through the existing `RigaEventEnvelope` path; it is not added to cross-run database persistence.

## Phase 2 handoff

The runtime now supplies graph events and readiness state. The next phase can promote the existing `.agent-work` region into the collapsible `RunDeck`, render summary counts and ready/blocked badges, and persist deck view state per session.
