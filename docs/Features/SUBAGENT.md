# Subagents

The subagent system is RIGA's orchestration layer: a parent orchestrator splits a
task across specialised child profiles through the `task` tool, and the runtime
tracks, bounds, and reports each child.

## Overview

- The **top-level run is the parent orchestrator**. It dispatches children and
  owns the final answer.
- A child is a **profile** — a named role with its own system rules, output
  format, default tool set, and model.
- Read-only profiles cannot write or dispatch; `build` can. This is enforced by
  the runtime, not by prompt alone.
- Dispatch is bounded by a depth limit (`MAX_TASK_DEPTH = 2`) and by the
  read-only boundary, so orchestration cannot fan out unboundedly.
- Every child is a first-class **outcome**: its result is recorded as evidence,
  and its failure becomes a lesson for the next run.

## Profiles

| Profile | Aliases | Model | Default tools | Can write / dispatch |
|---|---|---|---|---|
| `explore` | `scout`, `explorer` | cheapest capable (subagent model) | read, glob, grep, skill, bash | no / no |
| `plan` | `planner` | strong reasoning (subagent model) | read, glob, grep, skill | no / no |
| `build` | `executor`, `worker` | capable coding (main model) | read, write, glob, grep, bash, task, skill | yes / yes |
| `review` | `reviewer` | strong reasoning (subagent model) | read, glob, grep, skill, bash | no / no |

Every profile also receives the run-state tools: `update_plan`, `update_todos`,
`update_graph`, `set_model_budget`, `grant_tools`, `reset_tools`.

Design decisions:

- **`plan` has no `bash`.** With shell access a planner executed the commands in
  its own plan and looped on them instead of returning a plan. `explore` and
  `build` are the profiles that run commands; `explore` and `review` keep `bash`
  but each call is approval-gated.
- **The child model is chosen by `subagent_model_for`**: the configured
  `subagent_model` for read-only profiles, else the main model. A local run
  dispatches local children and a remote run dispatches remote children, so a
  child never crosses the provider boundary.
- **`plan` is told to return the plan and stop** rather than calling
  `update_plan`/`update_todos` repeatedly.

## The `task` tool

`task` is the tool name; `dispatch` is only an action value.

- `{"action": "dispatch", "agent": "explore", "prompt": "…", "description": "…"}`
  runs a child and returns its result.
- `{"action": "agents"}` lists the profiles. Listing is not progress.
- Durable actions (`create`, `inspect`, `update`, `list`) are cross-run
  bookkeeping and do not start a worker; they need configured secure persistence.
  Without it, use `update_plan`/`update_todos` for run-local progress.

A graph node is dispatched with `task` action `dispatch`, the matching `agent`,
and its `node_id`.

## Dispatch mechanics

`dispatch_subagent` is the single path for every child:

- **Depth limit** — `MAX_TASK_DEPTH = 2`; a deeper request is refused with a
  clear message.
- **Graph nodes** — when dispatched with a `node_id`, the node must belong to the
  current graph and match the profile, and its dependencies must be complete
  (`TaskBlocked` otherwise). Its state is written back to the graph
  (`Completed`/`Failed`).
- **Task records** — `TaskStarted`, `TaskStatus` (running/terminal), and
  `TaskCompleted { ok, result }` are emitted so RunDeck can render the child.
- **Tool set** — resolved by `allowed_tools_for(profile, grants)`; the child's
  `task` tool is only present if the profile permits it.
- **Nested loop** — the child runs the same local or remote loop as the parent,
  tagged with its task id so its tool calls nest in the UI.

## Read-only boundaries

A read-only child that calls a tool it does not have gets a **redirect, not a
fatal error**:

- the message tells it to return the requested result as prose and stop;
- the call does **not** count toward the consecutive-failure guard, so three
  disallowed calls no longer end the child's run;
- the per-tool cap (6) and the no-progress set guard still bound a model that
  keeps calling it.

This matters because a weak local model will try to `task`-dispatch even when it
cannot, and an earlier version killed the whole child on the third try — losing
the plan it was about to return.

A **record-only turn** (`update_plan`/`update_todos`/`update_graph` and nothing
else, with prose) ends a **subagent's** run, so a `plan` agent that records state
and writes its plan stops instead of looping. The **top-level orchestrator does
not** end there — recording the plan is how it moves on to dispatch the next
agent.

## Tool grants (HITL)

A subagent that genuinely needs a tool it lacks is granted it for the run, with
the user's approval:

```json
{"name": "grant_tools", "arguments": {"profile": "explore", "tools": ["webfetch"], "reason": "fetch upstream docs"}}
```

`reset_tools` clears one profile's grants or all of them. Grants are run-scoped,
never persist, and never edit the profile definition — so the user always sees
and approves the change.

## Model budget

`set_model_budget` sets a run-scoped budget that takes effect on the next turn,
so a prompt can use different parameters per step. Tiers:

| Tier | max_tokens | min_output | run_token_budget | max_turns | turn_seconds | no_progress_seconds |
|---|---:|---:|---:|---:|---:|---:|
| `compact` | 2,048 | 1,024 | 8,192 | 6 | 150 | 60 |
| `middle` (default) | 8,192 | 1,536 | 20,480 | 8 | 240 | 90 |
| `large` | 16,384 | 2,048 | 40,960 | 10 | 360 | 120 |

`enlarge` roughly doubles the numeric limits (+4 turns); `shrink` halves them,
floored at `compact`. Both clamp to hard ceilings, so they are safe to repeat.

## Evidence and knowledge for subagents

- Every subagent outcome is recorded. A completion or failure is emitted as
  evidence under the agent's name (`agent \`build\` failed: …`).
- A failure is summarised into a lesson at run end (`\`build\` failed: …`), which
  is injected into the next run in the session.
- The wrapper `task` tool is skipped in the ledger so a dispatch is not
  double-counted — the child records itself.

See `EVIDENCE.md` and `KNOWLEDGE.md`.

## Future work (ordered by least ROI)

1. **Per-profile evidence roll-up** — fold a child's tool evidence into one
   summary card. Low value: the child's cards are already visible and nested.
2. **Configurable profiles** — let a user define a profile in the workspace.
   Low-to-moderate: the four built-ins cover the common roles; a config file adds
   a trust and validation surface for little gain today.
3. **Parallel dispatch** — dispatch independent graph roots concurrently.
   Moderate: the graph already models readiness, but the single local engine
   serialises generation, so the win is small on local runs.
4. **A `plan`-style profile that can propose a graph** — have a child return a
   `update_graph` payload for the parent to publish. Moderate: `update_graph`
   already exists; the value is in a model that uses it well.
5. **Automatic profile selection** — pick the profile for a task from its shape.
   Moderate-high: saves the parent a decision, but the parent's choice is
   usually fine and cheap to correct.
6. **Cross-run subagent memory** — reuse a prior child's result for the same
   task. High: avoids re-running expensive recon, but needs task identity and
   staleness rules to avoid reusing stale results.
