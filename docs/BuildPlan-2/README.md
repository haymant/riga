# RIGA BuildPlan-2 RunDeck Runbook

This directory is the implementation handoff for the graph-native RIGA runtime. **RunDeck** is the operational job runner; **Run Deck** is the in-product execution UI. Rundeck jobs must invoke the repository’s deterministic commands, capture stdout/stderr and reports, and fail on non-zero exit codes.

## Agent prompt template: continue a phase

```text
Continue RIGA BuildPlan-2 Phase ${option.phase} in ${option.repo_dir} at ref ${option.ref}.

Read RIGABuildPlan-2.md and every prior phase report. Inspect git status and determine
whether the requested phase is complete before editing. Implement only the next
incomplete exit criterion. Preserve the kernel as the source of truth, keep adapters
thin, add focused Rust/Vitest tests, and do not invent a second agent loop.

Required report:
- Status: PASS or BLOCKED
- Exit criteria completed:
- Files changed:
- Tests and exact commands:
- Artifacts written:
- Remaining risks:
- Next phase allowed: YES or NO
```

## Phase 4 — Evidence graph

### Rundeck coding prompt

```text
Implement BuildPlan-2 Phase 4. Add EvidenceNode, EvidenceAdded, and EvidenceLinked
contracts in the kernel. Add a read-only evidence-producing path that preserves
source_ref and confidence. Render evidence cards in the Run Deck Evidence lens.
Reject empty source references and clamp or reject invalid confidence values.
Add Rust event/tool tests and Vitest render, empty-state, confidence, and source-link
interaction tests. Do not compute repository facts in React.
```

### Validation job

```bash
cargo fmt --all -- --check
cargo test --workspace evidence
cargo test --workspace events
npm test -- --run
npm run check
```

Verify that an evidence card displays a claim, `file:line` source reference, confidence, and optional task id; that an empty lens is explicit; and that the event survives journal replay.

## Phase 5 — Repository intelligence

### Rundeck coding prompt

```text
Implement BuildPlan-2 Phase 5. Add find_symbol, find_callers, find_references,
and find_tests as bounded, read-only kernel/server tools. Return deterministic
workspace-relative file:line evidence, ignore .git/node_modules/target/.riga,
cap files/results, and reject path escapes. Add fixture-repository tests for
symbol definitions, callers, references, tests, no-match behavior, and truncation.
Expose schemas in the catalog and keep the frontend as a display-only consumer.
```

### Validation job

```bash
cargo fmt --all -- --check
cargo test --workspace repository
cargo test --workspace catalog
npm run check
npm test -- --run
```

The job passes only when all four tools return `file:line` evidence on the fixture repository, do not descend into ignored directories, and produce stable ordering across repeated calls.

## Phase 6 — Knowledge graph

### Rundeck coding prompt

```text
Implement BuildPlan-2 Phase 6. Add KnowledgeNode, KnowledgeCreated, and
KnowledgeLinked contracts. Persist reusable facts in the run/session checkpoint
store with source-run and confidence metadata. Reuse a matching fact on a later
run without rescanning its source. Render knowledge cards in the Run Deck Knowledge
lens. Add Rust persistence/reuse tests and Vitest card/empty-state tests.
```

### Validation job

```bash
cargo fmt --all -- --check
cargo test --workspace knowledge
cargo test --workspace persistence
npm run check
npm test -- --run
```

Verify that a knowledge fact survives process restart, keeps its source run id and confidence, is reused without a repository scan, and is visibly linked to evidence where applicable.

## Full phase gate

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check
npm test
npm install --prefix demo
npm run build --prefix demo
cargo check --manifest-path demo/src-tauri/Cargo.toml --locked
```

In Rundeck, inspect the execution result, node, logs, uploaded test/coverage artifacts, and cleanup step. A job with skipped tests, unavailable artifacts, leaked processes, or a missing phase report is **BLOCKED**, not successful.

## Optional protected provider test

Do not place `OPENCODE_API_KEY` in options, logs, source, fixtures, or artifacts. Inject it only as a protected Rundeck project/node environment variable for a trusted job. Run a bounded connectivity/protocol smoke test and assert structural response validity, not exact model prose. The deterministic phase gates above must remain runnable without any provider secret.
