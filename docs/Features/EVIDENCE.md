# Evidence

Evidence is the run's **record of what actually happened**: the tools that ran,
the subagents that settled, and whether each succeeded. It backs the RunDeck
**Evidence** lens and is the raw material the knowledge summary is built from.

## Overview

- Evidence is **automatic**. The harness records every tool and subagent outcome;
  the model does not have to call anything for the lens to fill.
- It is deliberately **filtered**: failures, successful *mutating* calls, and
  subagent tasks are surfaced; reads and globs are logged for the summary but not
  emitted, so the lens keeps its signal.
- Each node carries a **source reference** and a **confidence**, so a claim can be
  traced and weighted.

## The evidence node

```rust
struct EvidenceNode {
    id: String,          // "ev-1", "ev-2", … (monotonic per run)
    claim: String,       // "`bash` failed: command not found"
    source_ref: String,  // "bash:task-1" or "bash:run"
    confidence: u8,      // 85 on success, 60 on failure
    task_id: Option<String>,
}
```

- **Claim** — for a tool, `bash failed: command not found`; for a subagent,
  `agent build failed: repeated the same bash call 3 times`. The summary is the
  first non-empty line of the result, capped at 140 characters.
- **Source reference** — `actor:task_id`, or `actor:run` for a top-level call, so
  the card points at the call that produced it.
- **Confidence** — 85 for a success, 60 for a failure. A failure is evidence too;
  it is just less certain to be actionable.
- **Task id** — set when the call belongs to a subagent, so evidence nests.

## Automatic recording

The run ledger (`RunEvidence`) keeps an ordered outcome log. Every tool and
subagent result is appended; an `EvidenceAdded` event is emitted for:

- **every failure** (any tool or subagent);
- **every successful mutating call** — `write`, `bash`, `edit`, anything whose
  tool risk is a workspace write or process execution;
- **every subagent task**, success or failure.

A successful read/glob/grep is logged for the run-end summary but not emitted.

Two deliberate exclusions:

- The wrapper **`task`** tool is skipped, because the dispatch path records the
  child under its own profile name — otherwise one dispatch would appear twice.
- The evidence for a child uses the child's profile name, so a `build` failure is
  attributed to `build`, not to `task`.

## Manual evidence

The model can still add evidence the harness cannot infer:

```json
{"name": "add_evidence", "arguments": {"claim": "the cache is unbounded", "source_ref": "src/cache.rs:88", "confidence": 70}}
{"name": "link_evidence", "arguments": {"claim_id": "claim-1", "evidence_id": "ev-1"}}
```

These emit `EvidenceAdded` / `EvidenceLinked` exactly like the automatic path, and
are meant to be used **in addition to** the automatic evidence, not instead of
doing the work.

## Events

| Event | Meaning |
|---|---|
| `EvidenceAdded` | a new evidence node |
| `EvidenceLinked` | an evidence node was linked to a claim |

Both are journaled and forwarded to every transport (IPC and WebSocket), so the
Evidence lens is identical in the desktop and web clients.

## Relationship to the other features

- **Knowledge** is derived from the same outcome log at run end. Evidence is the
  per-item record; knowledge is the lesson drawn from it.
- **Subagents** are first-class evidence, which is why a child's failure reaches
  the parent's summary without extra plumbing.

See `KNOWLEDGE.md` and `SUBAGENT.md`.

## Future work (ordered by least ROI)

1. **Evidence pagination** — a run with many tool calls can grow the lens. Low
   value: the card list already scrolls, and a run is bounded by the loop guards.
2. **Evidence search/filter** — filter by actor or success. Low: the list is short
   and already grouped by run.
3. **Evidence dedup** — collapse repeated identical cards. Low-moderate: the
   repeat guard already stops the model from re-running the same call, so
   duplicates are rare.
4. **Confidence from the source** — weight a subagent result by its model, a test
   by its exit code, and so on. Moderate: the 85/60 split is coarse but the extra
   signal is small and easy to get wrong.
5. **A non-zero pipeline is a failure** — mark a `bash` call failed when any
   stage of a pipeline fails or when it writes to stderr, not only when the last
   command's exit code is non-zero. Moderate-high: today a broken script can read
   as success (a `find` error behind a successful `head`), which misleads the
   model and the lens.
6. **Evidence as retry input** — feed the run's own evidence back into the loop
   when a subagent fails, so the model changes course mid-run rather than only on
   the next run. High: this is the gap that let an orchestrator improvise after a
   `plan` failure instead of delegating to `build`.
7. **A verified/unverified flag** — distinguish a claim backed by a successful
   mutating call from one that is only asserted. High: it is the same signal the
   `claims_work` guard uses, surfaced for the user.
