# BuildPlan-2 Phase 4 Report — Evidence graph

## Status

Implemented in the current working tree; validation is pending on a machine with Rust and installed npm workspace dependencies.

## Delivered

- Added kernel `EvidenceNode`, `EvidenceAdded`, and `EvidenceLinked` contracts.
- Added `add_evidence` and `link_evidence` server-internal tool paths.
- Added RunDeck Evidence lens cards showing claim, source reference, confidence, and task id.
- Added explicit empty-state rendering and theme-aware evidence card styling.

## Exit-criterion mapping

- Claims carry `source_ref` and bounded confidence: implemented.
- Evidence cards render in the Run Deck: implemented.
- Evidence events use the durable event protocol: implemented through `RigaEvent` and normal journal/broadcast handling.

## Required validation

```bash
cargo test --workspace
npm test
npm run check
```
