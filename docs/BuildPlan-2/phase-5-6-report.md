# BuildPlan-2 Phases 5–6 Report — Repository intelligence and knowledge graph

## Status

Implemented in the current working tree; full Rust and frontend validation is pending because this sandbox checkout has `npm` but no `cargo`, and workspace node dependencies are not installed.

## Phase 5 delivered

- Added catalog entries for `find_symbol`, `find_callers`, `find_references`, and `find_tests`.
- Added bounded, read-only repository queries returning workspace-relative `file:line:text` evidence.
- Reused the existing ignored-directory, file-size, match-count, and result-size safeguards.
- Added empty-result behavior and non-empty query validation.

## Phase 6 delivered

- Added kernel `KnowledgeNode`, `KnowledgeCreated`, and `KnowledgeLinked` contracts.
- Added `remember` and `link_knowledge` server-internal tool paths.
- Added RunDeck Knowledge lens cards with fact, confidence, and source-run metadata.
- Enabled Evidence and Knowledge lens selection instead of leaving them disabled.

## Required validation

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm install
npm run check
npm test
```

The remaining persistence/reuse portion of Phase 6 should be completed only after the repository’s checkpoint schema is selected; the current implementation establishes the event and UI contract without inventing a second persistence store.
