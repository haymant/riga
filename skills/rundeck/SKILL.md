---
name: rundeck
description: Orchestrate RIGA subagents with RunDeck graphs, dependencies, live-status interpretation, active-run recovery, cancellation controls, evidence and knowledge lenses, and concise profile-specific prompts. Use when coordinating explore, plan, build, or review agents; checking running/ready/blocked work; reading the Evidence or Knowledge lens; recovering a stale or looping local run; or designing a multi-stage repository task.
---

# RunDeck orchestration

Use this skill for live subagent coordination. Keep durable task bookkeeping separate from live execution.

## Core model

- The top-level RIGA run is the parent orchestrator.
- The parent model dispatches child profiles through the `task` tool.
- `plan`, `explore`, and `review` are read-only and cannot dispatch nested agents.
- `build` may write files, run commands, and dispatch children when its profile permits it.
- RunDeck is the in-product live execution view; durable `task` records are cross-run bookkeeping.
- A second top-level run cannot start while a local run is active. Stop or wait for the current run first.
- The harness records every tool and subagent outcome as **evidence**, and summarises each run into **knowledge** — you do not need to call `add_evidence`/`remember` for those lenses to fill.
- Prior lessons from the session are injected into your prompt as a `[Prior knowledge …]` block; treat them as facts and apply them instead of relearning them.

## Parent-coordinated workflow

When the user asks to study first and then plan or implement, act as the parent:

1. Decide whether stages have dependencies.
2. For dependent stages, call `update_graph` once with a small DAG.
3. Dispatch only the root node first, or dispatch independent roots in parallel.
4. After a dependency completes, dispatch the newly ready node and include the prior result in its prompt.
5. Report running, ready, blocked, completed, and failed work concisely.

Preferred explore-to-plan prompt:

```text
Act as the parent coordinator. Use update_graph to publish:
- explore: inspect the repository and collect evidence
- plan: propose the highest-ROI enhancements, depending on explore
Then dispatch explore. When explore completes, pass its findings to plan and dispatch plan. Do not modify files. Return a prioritized plan with impact, effort, files, tests, and risks.
```

Do not ask a read-only child to dispatch another child. The parent must perform the next dispatch.

## Exact tool forms

Use `task` as the tool name. `dispatch` is an action value, not a tool name:

```json
{
  "action": "dispatch",
  "agent": "explore",
  "prompt": "Inspect the repository and summarize the relevant implementation. Do not modify files.",
  "description": "Repository reconnaissance"
}
```

For a graph, use unique IDs, known profiles, prompts, and dependency edges:

```json
{
  "title": "Explore and prioritize enhancements",
  "nodes": [
    {"id":"explore","profile":"explore","description":"Inspect repository","prompt":"Collect evidence for high-ROI improvements.","depends_on":[]},
    {"id":"plan","profile":"plan","description":"Rank improvements","prompt":"Use explore findings to rank improvements by impact, effort, files, tests, and risks.","depends_on":["explore"]}
  ]
}
```

Dispatch a graph node with `task` action `dispatch`, the matching `agent`, and its `node_id`. Pass completed findings in the dependent prompt; child context is not shared automatically.

Use `task action="agents"` only to list profiles. Listing profiles is not progress and does not start a worker.

## Active runs, status, and cancellation

Open the RunDeck **Execution** lens to inspect:

- **running**: a worker is executing;
- **ready**: dependencies are satisfied;
- **blocked**: one or more dependencies are incomplete or failed;
- **completed**: the worker returned successfully;
- **failed**: the worker returned an error;
- **cancelled**: cancellation won over a late result.

The RunDeck UI exposes **Active runs** after connecting or reconnecting. Expand RunDeck to list each live run, its run ID, session, and whether it uses the local engine. Use the row's **Stop** button to cancel that run; **Stop active run** cancels the current run and its child agents.

For `local_run_in_progress`, do not start another run. Open RunDeck, inspect the active run, then stop it or wait for completion. If the deck is collapsed, click its RunDeck summary in the composer footer.

A live task is not the same as a durable task record. Durable actions (`create`, `inspect`, `update`, `list`) do not start a worker and require configured secure persistence. If unavailable, use `update_plan` and `update_todos` for run-local progress.

## Evidence and knowledge (automatic)

You do not record anything for the **Evidence** and **Knowledge** lenses to work —
the harness derives them from what actually ran.

- **Evidence** (`EvidenceAdded`): every tool and subagent outcome is logged. The
  lens surfaces failures, successful mutating calls (`write`/`bash`/`edit`), and
  subagent tasks. Each card reads `actor succeeded|failed: first-line summary`,
  with a `source_ref` (`actor:task-…`) and a confidence (85 success / 60 failure).
  Reads and globs are logged for the run summary but not surfaced, so the lens
  keeps its signal.
- **Knowledge** (`KnowledgeCreated`): at run end the outcomes are summarised into
  lessons — repeated failures, individual failures, and what each subagent
  achieved. The lens shows them with their source run.
- **Retry context**: lessons persist per session and are prepended to the next
  run's prompt:

  ```text
  [Prior knowledge for this session — lessons from earlier runs. Apply them; do not relearn them.]
  - `bash` failed 2× in a row (latest: …). Do not repeat an identical call — vary the approach or report what you have.
  [End prior knowledge]
  ```

  When you see this block, **act on it**: avoid the approach a lesson says failed,
  prefer the one it says worked, and do not spend tools re-deriving it.

`add_evidence` and `remember` still exist for facts the harness cannot infer — a
claim with a `source_ref` you chose, or a durable fact. Use them *in addition to*
the automatic evidence, not instead of doing the work.

## Profile boundaries

| Profile | Model | Default tools | Can write/dispatch? |
|---|---|---|---|
| `explore` / `scout` | cheap/fast (subagent model) | read, glob, grep, skill, bash | No / No |
| `plan` / `planner` | strong reasoning (subagent model) | read, glob, grep, skill | No / No |
| `build` / `executor` | capable coding (main model) | read, write, glob, grep, bash, task, skill | Yes / Yes |
| `review` / `reviewer` | strong reasoning (subagent model) | read, glob, grep, skill, bash | No / No |

Every profile also gets the run-state tools: `update_plan`, `update_todos`,
`update_graph`, `set_model_budget`, `grant_tools`, `reset_tools`.

`bash` is available to `explore`, `review`, and `build`, but each call is
**approval-gated**, so a recon agent can run `find`/`ls`/`git` once you approve.
`plan` deliberately has **no shell**: a planner that can execute loops on its own
plan's commands instead of returning a plan — dispatch `build` to run them.
`write` and `task` are only on `build`. The model is chosen by
`subagent_model_for`: the configured `subagent_model` for read-only profiles,
else the main model.

### Granting tools to a subagent (HITL)

When a subagent genuinely needs a tool it does not have (for example `webfetch`
for `explore`), the parent grants it for this run — with your approval:

```json
{"name": "grant_tools", "arguments": {"profile": "explore", "tools": ["webfetch"], "reason": "fetch upstream docs"}}
```

The runtime asks you to confirm; on approval the tool joins that profile's set
for the rest of the run. Reset with:

```json
{"name": "reset_tools", "arguments": {"profile": "explore"}}
{"name": "reset_tools", "arguments": {}}
```

`reset_tools` with no profile clears every grant. Grants are run-scoped and do
not persist. Never broaden a read-only profile by editing its definition; use
`grant_tools` so the user sees and approves the change.

## Model budget (compact / middle / large)

The local run has a **run-scoped budget** you can change with `set_model_budget`.
It takes effect on the next turn, so a prompt can use different parameters per
step. The budget includes reasoning, tool-call JSON, and the final answer.

Pick a capability tier, or resize the current one:

```json
{"tier": "compact"}
{"tier": "middle"}
{"tier": "large"}
{"enlarge": true}
{"shrink": true}
{"max_tokens": 16384, "run_token_budget": 40960, "turn_seconds": 360}
```

Exact tier parameters:

| Tier | max_tokens | min_output | run_token_budget | max_turns | turn_seconds | no_progress_seconds |
|---|---:|---:|---:|---:|---:|---:|
| `compact` | 2,048 | 1,024 | 8,192 | 6 | 150 | 60 |
| `middle` (default) | 8,192 | 1,536 | 20,480 | 8 | 240 | 90 |
| `large` | 16,384 | 2,048 | 40,960 | 10 | 360 | 120 |

- `max_tokens` — per-turn output cap (clamped to the context room).
- `min_output` — output reserve; the prompt is trimmed until it fits.
- `run_token_budget` — summed across every turn.
- `max_turns`, `turn_seconds`, `no_progress_seconds` — loop bounds.
- `enlarge` doubles the numeric limits (+4 turns); `shrink` halves them. Both
  clamp to hard ceilings, so they are safe to call repeatedly.
- `context` is a **load-time** setting (`RIGA_LOCAL_CONTEXT`); changing it needs
  a model reload, so it is not part of this tool.

Guidance: `compact` for cheap recon on a ~2B model, `middle` for 3–8B, `large`
for ≥13B or a reasoning model. Raise `max_tokens` before blaming the model when
a `write` body or a tool call is truncated; do not shrink the build budget to
fix verbose planning.

Pair it with a prompt-level limit:

```text
Use tools only until the requested evidence is collected. Return only the requested sections. Limit the final answer to 500 words and 30 lines. Do not repeat tool output, speculate beyond evidence, or reveal private chain-of-thought.
```

## Recovering a failed run

A run can fail with a clear message. Read it, adjust, then retry — do not
re-dispatch the same way:

- **`the local model kept emitting a tool call that is not valid JSON; it may
  need a larger output limit or a smaller request`** — the tool call was cut off
  or malformed. Enlarge the budget, then retry:
  ```json
  {"name": "set_model_budget", "arguments": {"enlarge": true}}
  ```
  and re-dispatch with an instruction to emit **one smaller call** (prefer
  several small `write` calls over one large one).
- **`hit the per-turn time limit while still generating`** — slow, not stuck.
  Enlarge `turn_seconds` (or use `large`), then retry.
- **`produced no token within the idle window`** — a real stall; retry once, and
  if it repeats use a smaller prompt or a smaller model.
- **`reached the N-turn budget` / `exceeded the N-token budget`** — enlarge the
  budget (`{"enlarge": true}`) or split the task into more graph nodes.
- **`the local model repeated the same tool set … N turns in a row`** — you are
  re-dispatching the same shape of work. Change the **plan**, not just the
  arguments: dispatch a different profile, narrow the prompt, or answer.
- **`the local model called … N times in this run without finishing`** — one tool
  is looping with *varying* arguments (a fresh `find`, `bash`, `read`, …). Stop
  retrying it; use a different tool or a narrower request.
- **`the local model repeated the same … call N times without finishing`** — that
  exact call already ran and its result is in the history. Use that result.
- **`the local model called … N times in a row and each attempt failed`** — the
  call cannot succeed as written. Fix the input or switch tools.
- **`the local model made more than 16 tool calls without finishing`** — the run
  is too broad; split it into graph nodes.
- **`` `X` is not available to this agent ``** — the profile lacks the tool. This
  is a redirect, not a fatal error: return the requested result as prose. If the
  tool is genuinely needed, `grant_tools` it (with approval) instead of
  re-dispatching the same way.
- **`task node is blocked by incomplete dependencies`** — dispatch the
  dependencies first; do not force the node.

### Loop guards (the harness stops these)

| Guard | Limit | What it means |
|---|---|---|
| Identical call | 2 repeats | that exact call already ran; use its result |
| Same tool, varying args | 6 calls | one tool is looping; switch approach |
| Same tool set per turn | 3 turns | you keep re-dispatching the same shape |
| Consecutive failures | 3 | the call cannot succeed as written |
| Whole run | 16 calls | the run is too broad; split it |
| Turn budget | tier | enlarge or split the task |

Each guard ends the run with a message naming the pattern, and the outcome is
recorded as evidence, so the next attempt starts with the lesson.

After a failure, `update_todos` the retry so the RunDeck shows the recovery.

## Validation checklist

- Confirm graph IDs are unique and dependencies are acyclic.
- Dispatch only ready nodes.
- Pass completed findings into dependent prompts.
- Check RunDeck for task ID, state, nested tool events, and result summary.
- Read the **Evidence** lens for what actually ran, and the **Knowledge** lens for
  the lessons before re-dispatching.
- On a retry, apply the `[Prior knowledge …]` block instead of repeating the
  approach a lesson says failed.
- Use Active runs to recover a run after reconnect or stale UI state.
- Use Stop on the specific run rather than starting a competing local run.
- Never place API keys, provider secrets, uploaded attachments, or generated model files in prompts or documentation.

Useful checks:

```bash
npm run check
npm test -- --run packages/assistant-ui/src/AssistantUI.test.tsx
cargo test -p riga-server
cargo test -p riga-kernel task
```
