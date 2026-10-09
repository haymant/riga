# BuildPlan-2 Phase 2 Report — RunDeck shell

## Status

**Complete.** The existing plan, todo, and subagent cards are now composed inside a first-class collapsible RunDeck.

## Implemented

- Promoted the legacy `.agent-work` region to a `RunDeck` shell.
- Added a compact summary header with:
  - completed/total task count;
  - currently running agent;
  - ready and blocked graph-node counts;
  - active graph lens and run scope.
- Added collapse/expand behavior with a persisted per-session local-storage key:
  - `riga.run-deck.<session-id>.collapsed`.
- Added mobile behavior:
  - narrow viewports default to collapsed;
  - expanded deck displays as an overlay above the composer/transcript;
  - deck remains usable without permanently consuming the chat viewport.
- Added graph lens controls:
  - Execution is active;
  - Evidence and Knowledge are visible as disabled future lenses until their event payloads exist.
- Added run-scope picker and a focus-node breadcrumb for execution-node drill-down.
- Wired Phase 1 events into the UI:
  - `GraphUpdated`;
  - `TaskDependencyAdded`;
  - `TaskRunnable`;
  - `TaskBlocked`;
  - `TaskStatus`;
  - existing `TaskStarted` and `TaskCompleted`.
- Expanded subagent rows to display pending, waiting, blocked, running, done, and failed states, including blocked-by dependencies and optional progress bars.
- Corrected the completion denominator so pending and blocked tasks are not counted as complete.

## Test evidence

```text
npm test
```

Passed:

- 5 test files
- 42 tests
- Existing HTTP and Tauri transport tests remain green
- AssistantUI tests cover:
  - running subagent rendering;
  - nested task-scoped tool calls;
  - completed and failed subagent results;
  - RunDeck summary rendering;
  - collapse/expand interaction;
  - per-session collapse persistence;
  - execution lens and disabled future lenses;
  - execution-node drill-down and breadcrumb;
  - run-scope selection.

```text
npm run check
```

Passed naming lint and TypeScript validation.

```text
cargo test --workspace
```

The Phase 1 Rust graph/readiness implementation remains covered by the previously recorded workspace test evidence; this phase changes only the React UI and CSS layer.

## Phase 3 handoff

The execution graph fallback list is in place. Phase 3 can add the pure graph adapter and XYFlow boundary without changing RunDeck ownership, lens controls, or scope state.
