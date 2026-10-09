# BuildPlan-2 Phase 7 — Adaptive coordinator context harness

## Status

**Planned.** This phase defines the implementation contract for the coordinator harness. It must be completed before adding automatic coordinator-context management to the runtime.

## Purpose

Prevent the coordinator model from becoming a context bottleneck when it dispatches multiple subagents. The coordinator must receive compact, relevant handoffs while the complete child result, tool trace, evidence, and artifacts remain available out-of-band through RunDeck and task inspection.

This design must work for small local models such as Qwen 4B and must not degrade larger models such as DeepSeek 4 Pro. The harness is therefore **lossless and adaptive**: it changes the coordinator's injected view, never destroys the canonical result.

## Non-goals

- Do not create a second agent loop or scheduler.
- Do not delete, rewrite, or irreversibly summarize the canonical child result.
- Do not infer repository facts in React.
- Do not require a provider API key for deterministic tests.
- Do not classify models by name alone when explicit capability metadata is available.
- Do not place full child output into coordinator reasoning text by default.

## Current bottleneck

The dispatch path currently returns child output as a tool result similar to:

```text
[explore subagent result]
<full child response>
```

That response becomes part of the coordinator conversation. Repeated dispatches therefore accumulate user context, system instructions, tool JSON, graph updates, and complete child responses. Small models lose quality before the task itself becomes large.

The current `dispatch_subagent` path in `crates/riga-server/src/ws.rs` must be changed so that:

1. the complete result is stored in the task/run evidence record;
2. a bounded structured handoff is returned to the coordinator;
3. a task-completion event points the UI to the full result;
4. reasoning text receives only a short progress statement, never the full result.

## Required data model

Add a server-side policy type. Exact names may vary only if the semantics remain identical:

```rust
struct CoordinatorContextPolicy {
    tier: ModelTier,
    context_window_tokens: usize,
    max_coordinator_tokens: usize,
    handoff_tokens: usize,
    max_retained_handoffs: usize,
    raw_result_policy: RawResultPolicy,
    compact_at_ratio: f32,
    emergency_at_ratio: f32,
}

enum ModelTier {
    Compact,
    Balanced,
    Large,
}

enum RawResultPolicy {
    Never,
    RelevantOnly,
    Allowed,
}
```

Add a canonical task-result envelope. The full result and the handoff must be separately addressable:

```json
{
  "task_id": "explore",
  "run_id": "run-123",
  "agent": "explore",
  "status": "completed",
  "summary": "One sentence, maximum 35 words.",
  "findings": [
    {"text": "Concrete fact.", "source": "crates/riga-server/src/ws.rs:2481"}
  ],
  "files": ["crates/riga-server/src/ws.rs"],
  "risks": ["One bounded risk."],
  "next": "One sentence for the parent.",
  "full_result_ref": "task://run-123/explore/result",
  "artifact_refs": [],
  "truncated": false
}
```

`full_result_ref` is mandatory for completed and failed tasks. A missing or inaccessible reference is a runtime error, not a silent omission.

## Model capability resolution

Resolve the coordinator policy in this order:

1. Explicit provider/model capability configuration.
2. A built-in model capability registry.
3. Provider metadata if the adapter exposes it.
4. Conservative fallback to `Balanced`.

Unknown model names must never default to `Large`.

The configuration must support explicit overrides for:

- context window;
- maximum coordinator context;
- tier;
- handoff budget;
- raw-result policy.

The coordinator model and `subagent_model` are independent. Resolve one policy for the parent coordinator and one handoff-output policy for child agents. A large child model must not force a compact coordinator to ingest large raw output.

Recommended defaults:

| Tier | Example | Handoff | Retained handoffs | Raw result injection | Compact threshold | Emergency threshold |
|---|---|---:|---:|---|---:|---:|
| Compact | Qwen 4B local | 700 tokens | 2 | Never | 0.60 | 0.80 |
| Balanced | unknown/standard | 1,100 tokens | 4 | RelevantOnly | 0.70 | 0.85 |
| Large | DeepSeek 4 Pro | 1,800 tokens | 8 | RelevantOnly | 0.75 | 0.90 |

These are defaults, not hardcoded model-name rules. All limits must be overridable for tests and deployments.

## Handoff contract

Every child prompt must require a bounded final handoff. The server must also enforce the bound after the child returns; prompts alone are insufficient.

### Explore/review handoff

```text
STATUS: completed | blocked | failed
SUMMARY:
One sentence, maximum 35 words.
FINDINGS:
- Maximum 5 bullets; each must contain one concrete fact and file/path evidence.
FILES:
- Maximum 6 workspace-relative paths.
RISKS:
- Maximum 3 bullets.
NEXT:
One sentence for the parent coordinator.

Hard limit: 700 tokens for Compact, 1,100 for Balanced, 1,800 for Large.
Do not repeat the task, tool output, private reasoning, or instructions.
```

### Plan handoff

```text
STATUS: completed | blocked | failed
GOAL: one sentence
PLAN: maximum 6 numbered steps
FILES: maximum 8 paths
TESTS: maximum 6 commands
RISKS: maximum 3 bullets
NEXT: one sentence
```

### Build handoff

```text
STATUS: completed | blocked | failed
FILES_CHANGED: maximum 12 paths
TESTS: exact commands and pass/fail result
BLOCKERS: maximum 3 bullets
NOTES: maximum 5 bullets
NEXT: one sentence
```

If a child returns prose instead of the contract, deterministic compaction must extract known headings and truncate by item count and character/token budget. It must preserve the original prose in the full-result store.

## Context assembly rules

Do not use one ever-growing transcript as the coordinator's sole memory. Assemble each coordinator request from separate sources:

```text
system prompt
+ current user objective
+ current graph state
+ active task and latest handoff
+ selected recent handoffs
+ explicitly retrieved evidence
+ bounded recent coordinator/tool history
```

The coordinator context must not automatically include:

- full child responses;
- repeated tool output already represented by a handoff;
- stale graph snapshots;
- duplicate task-start/task-complete prose;
- reasoning deltas from previous turns.

The server must retain only the latest graph state in the active prompt. Graph events remain durable and visible to RunDeck.

## Full-result retrieval

Add an explicit task inspection path with summary/full modes. The exact protocol may reuse the existing `task` tool:

```json
{"action":"inspect","task_id":"explore","detail":"summary"}
{"action":"inspect","task_id":"explore","detail":"full"}
{"action":"inspect","task_id":"explore","detail":"sections","sections":["findings","risks"]}
```

Default `inspect` detail is `summary`. Full output requires an explicit request or a relevance decision by the harness. Retrieval must be bounded by the current policy and must return `full_result_ref` even when content is truncated.

The coordinator prompt should teach:

```text
Full child results are stored out-of-band. Use task inspect only when the compact handoff lacks a required detail. Do not request full results for every completed task.
```

## Context-pressure adaptation

Before every coordinator model request, estimate token usage using the actual serialized request shape. Track at least:

```text
system prompt
+ user messages
+ graph state
+ retained handoffs
+ selected evidence
+ tool history
```

Apply these actions:

| Usage ratio | Required behavior |
|---:|---|
| below compact threshold | Normal policy; retain tier-defined handoffs. |
| compact threshold to 80% | Drop duplicate tool output and stale graph snapshots; keep summaries and active task. |
| 80% to emergency threshold | Retain graph state, current objective, active task, and latest two handoffs only; mark omitted results retrievable. |
| above emergency threshold | Compact before dispatching another child; do not send the provider request until below threshold. |

Never silently discard a result. Omitted content must remain retrievable by task ID.

## Dispatch result behavior

Replace raw result injection at every dispatch completion path, including sequential, fan-out, and local-model parsing paths. Do not fix only one branch.

Required sequence:

1. Validate the child result status.
2. Persist the full result and tool trace under the task/run record.
3. Produce and validate the bounded handoff envelope.
4. Emit durable task completion/status events containing the envelope and reference.
5. Return only the bounded handoff as the `task` tool result.
6. Emit a short ephemeral coordinator progress event, for example:
   `explore completed; 4 findings stored; full result available by task id.`

The full child output must not be copied into `ReasoningDelta` or any coordinator-facing transcript item.

## Large-model behavior

The harness must not artificially force every model into Qwen-sized context. For `Large`, it may:

- retain more handoffs;
- return a larger bounded handoff;
- retrieve relevant sections automatically when the current task references the child’s files or topic;
- preserve more cross-task evidence before compaction.

It must still avoid unconditional concatenation of all raw results. Larger context is an allowance, not a requirement.

## Failure and fallback rules

- Unknown model capability: use `Balanced`.
- Missing context-window metadata: use configured conservative default and record a diagnostic.
- Malformed child handoff: store full output, produce a deterministic fallback summary, mark `truncated: true`, and continue.
- Full-result storage failure: mark the task `failed` and do not claim successful handoff persistence.
- Token estimator failure: use the emergency compact policy, never send unbounded context.
- Provider context rejection: retry once with emergency compaction; do not retry unchanged.
- Child failure: return a bounded failure handoff with error category and full-result reference; do not include a full stack trace in coordinator context.
- Cancellation: preserve the partial result and mark it cancelled; it remains inspectable.

## UI requirements

RunDeck must display the compact handoff by default and provide an explicit “View full result” action. The UI must show:

- task id and profile;
- status;
- summary/findings;
- truncation indicator;
- full-result availability;
- artifact/evidence references.

Full result viewing must not mutate the coordinator context. It is a UI retrieval operation.

## Settings and provider configuration requirements

The settings popover must use one coherent visual system:

- rounded paper/surface card with a restrained border and no unrelated heavy shadow;
- mono, muted section labels;
- assistant-ui settings-panel-like underline tabs with `aria-selected` or equivalent state;
- field surfaces using the same border, radius, focus ring, and text tokens;
- one shared action-button family for neutral, destructive, and primary actions;
- permission-grant-like action hierarchy: neutral deny/cancel, outlined session action, filled primary/persistent action;
- “Save settings” remains required because provider credentials and configuration are persisted; its label must describe persistence and it must use the same primary button family as other approval/save actions.

## Test plan

### Rust/server tests

Add deterministic tests for:

1. capability resolution precedence;
2. unknown-model Balanced fallback;
3. tier default budgets;
4. explicit policy overrides;
5. structured handoff parsing;
6. malformed handoff fallback;
7. item-count and token/character truncation;
8. full-result reference persistence;
9. no raw child result in coordinator reasoning text;
10. all dispatch completion branches returning compact handoffs;
11. graph-state-only compaction;
12. context threshold transitions;
13. provider context rejection retry with emergency compaction;
14. cancellation and failed-task references;
15. summary/full/section task inspection;
16. local and remote coordinator behavior.

### Frontend tests

Add Vitest/Testing Library tests for:

1. compact handoff rendering;
2. full-result retrieval action;
3. truncated-result indicator;
4. task status and artifact links;
5. settings tab selected state and keyboard navigation;
6. shared primary/neutral/deny button classes;
7. save-settings action remains visible and consistent;
8. permission-style approval hierarchy.

### Required commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check
npm test -- --run
npm run build --prefix demo
```

## Exit criteria

Phase 7 is complete only when all are true:

- no dispatch completion path injects full child output into coordinator reasoning;
- every completed/failed/cancelled child has a full-result reference;
- compact handoff limits are enforced server-side;
- capability policy adapts by resolved model tier;
- unknown models use Balanced policy;
- context pressure triggers deterministic compaction before provider failure;
- full results are inspectable on demand from RunDeck/task inspection;
- graph state remains authoritative and compact;
- local Qwen-sized runs pass without context overflow in fixture tests;
- large-model policy tests prove larger budgets without unconditional raw injection;
- settings popover and permission actions use the unified button/tab language;
- all required validation commands pass;
- this phase report records exact files, tests, and remaining risks.

## Implementation order

1. Introduce capability/policy types and deterministic budget helpers.
2. Introduce full-result references and task inspection storage.
3. Introduce structured handoff parsing and bounded fallback compaction.
4. Replace raw dispatch-result injection in every dispatch path.
5. Add context assembly and pressure compaction.
6. Add provider rejection retry with emergency policy.
7. Add RunDeck full-result retrieval UI.
8. Finish settings/button unification and tests.
9. Run the full phase gate and write the phase report.
