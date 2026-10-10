# Knowledge

Knowledge is the **reusable lesson** a run leaves behind. Where evidence records
each outcome, knowledge summarises the outcomes into facts the next run should
start from, so a retry does not rediscover what a failure already taught.

## Overview

- Knowledge is **derived automatically** from the run's outcome log at run end —
  the same log that backs evidence.
- It is **persisted per session** and **injected into the next run's prompt**, so
  a retry starts from prior lessons.
- The summary is **deterministic**, not a model call: cheap, reliable on small
  models, and testable.

## The knowledge node

```rust
struct KnowledgeNode {
    id: String,           // "k-1", "k-2", …
    fact: String,         // "`bash` failed 2× in a row …"
    source_run_id: String,
    confidence: u8,
}
```

## Derivation rules

At run end the outcome log is summarised into lessons:

| Rule | Lesson | Confidence |
|---|---|---|
| Repeated failure of one actor (≥2 in a row) | ``bash failed 2× in a row (latest: …). Do not repeat an identical call — vary the approach or report what you have.`` | 75 |
| Any failure | ``build failed: repeated the same bash call 3 times…`` | 65 |
| A subagent that completed | ``plan completed: ### Goal …`` | 80 |

- Lessons are **deduplicated** by fact, so the same failure on two turns yields
  one card.
- The repeated-failure rule is the one that names the *loop pattern* the guards
  stopped — the most actionable lesson a weak model can receive.
- An individual failure is kept even when it is not repeated, because "this tool
  does not work here" is worth remembering.

## Persistence

Lessons are written to:

```
<data dir>/knowledge/<session>.json
```

- `<data dir>` is `RIGA_DATA_DIR`, else `$HOME/.local/share/riga`, else
  `.riga-data`.
- The file is **merged**, not overwritten: an existing fact is kept once, and the
  file is **capped at 64 facts** (oldest dropped first).
- Each fact records the run that produced it (`source_run_id`).

## Injection into the next run

At the start of a run, the session's knowledge is loaded and prepended to the
prompt (newest-first, capped at 12 facts):

```text
[Prior knowledge for this session — lessons from earlier runs. Apply them; do not relearn them.]
- `bash` failed 2× in a row (latest: …). Do not repeat an identical call — vary the approach or report what you have.
- `build` failed: repeated the same bash call 3 times…
[End prior knowledge]
```

The block is a nudge, not a transcript — short, and explicitly framed so the model
applies the lesson instead of re-deriving it. It goes into the **user** prompt;
the system prompt is unchanged.

## Manual knowledge

The model can record a durable fact the harness cannot infer:

```json
{"name": "remember", "arguments": {"fact": "the build uses pnpm, not npm", "confidence": 80}}
{"name": "link_knowledge", "arguments": {"knowledge_id": "k-1", "evidence_id": "ev-3"}}
```

These emit `KnowledgeCreated` / `KnowledgeLinked` like the automatic path. The
automatic summary is the baseline; `remember` is for facts that outlive a run.

## Events

| Event | Meaning |
|---|---|
| `KnowledgeCreated` | a new knowledge node (automatic at run end, or `remember`) |
| `KnowledgeLinked` | a knowledge node was linked to an evidence node |

## Relationship to the other features

- **Evidence** is the per-outcome record; **knowledge** is the lesson drawn from
  it. They share one outcome log, so they can never disagree about what ran.
- **Subagents** contribute their own outcomes, so a child's failure becomes a
  session lesson for the parent.

See `EVIDENCE.md` and `SUBAGENT.md`.

## Future work (ordered by least ROI)

1. **Knowledge expiry** — drop facts older than N runs. Low: the file is capped
   at 64 and deduped, so stale facts are bounded.
2. **Per-fact pinning** — keep a fact forever. Low: `remember` already exists for
   durable facts.
3. **Richer card fields** — show the actor and confidence on the card. Low: the
   fact text already names the actor.
4. **Knowledge scoping** — scope to an agent, a workspace, or a task type rather
   than a session. Moderate: session scope is simple and mostly right; broader
   scope needs a relevance model to avoid injecting noise.
5. **A model-written summary** — have the model phrase the lesson. Moderate: the
   deterministic rules are reliable and free; a model call adds latency and can
   hallucinate a lesson the evidence does not support.
6. **A validated knowledge base** — require a lesson to be confirmed by a later
   success before it is trusted. Moderate-high: raises precision, but needs a
   notion of "the same task" that the runtime does not yet have.
7. **Mid-run lesson injection** — feed the current run's lessons back into the
   loop as they are derived, not only on the next run. High: this is what would
   have made the orchestrator change course after a `plan` failure instead of
   improvising.
8. **Cross-session knowledge** — share lessons across sessions for the same
   workspace. High: a repository's quirks are not session-specific, and relearning
   them every session is the main cost today.
