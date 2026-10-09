# BuildPlan-2 Phase 0 Report — UI test harness

## Status

**Complete.** Phase 0 entry and exit criteria were satisfied on the current `main` branch.

## Scope verified

- Vitest discovers both transport tests and React UI tests under `packages/assistant-ui/src/`.
- `jsdom`, `@testing-library/react`, and `@testing-library/jest-dom` are declared in the root development dependencies.
- `packages/assistant-ui/src/AssistantUI.test.tsx` exercises the assistant UI with a fake transport.
- The UI test covers a running subagent, nested task-scoped tool calls, streamed tool output, completed results, and failed results.
- Existing HTTP and Tauri transport suites remain in the same test run.
- Coverage thresholds remain enforced at lines/functions/statements ≥ 80% and branches ≥ 75% for the currently included transport modules.

## Validation evidence

Executed from the repository root:

```text
npm test
```

Result:

- 5 test files passed
- 41 tests passed
- Coverage: 89.15% statements, 94.09% lines, 86.71% functions, 78.70% branches
- All configured thresholds passed

```text
npm run check
```

Result:

- RIGA naming lint passed
- Root TypeScript check passed

```text
cargo test --workspace
```

Result:

- `riga-kernel`: 29 passed
- `riga-server`: 88 passed
- `riga-shell`: 1 passed
- All workspace doc tests passed

## Environment note

The sandbox initially had no usable Rust command in `PATH` and had declared npm dependencies without installed `node_modules`. The frontend dependencies were installed with `npm install --ignore-scripts`; a current Rust stable toolchain was installed through rustup because the distro Cargo version did not support the repository's Edition 2024 manifests. No repository source or lockfile changes were required for these environment repairs.

## Phase 1 handoff

The repository is ready for the graph payload and readiness work:

- Add `Graph` / `GraphNode` validation in `riga-kernel`.
- Add `update_graph` as run-state tooling.
- Add graph and readiness events.
- Reject dispatches whose dependencies are incomplete.
- Emit `TaskStatus` transitions and journal authoritative graph events.
