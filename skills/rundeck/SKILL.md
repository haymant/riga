---
name: rundeck
description: Orchestrate RIGA subagents with RunDeck graphs, dependencies, live-status interpretation, active-run recovery, cancellation controls, and concise profile-specific prompts. Use when coordinating explore, plan, build, or review agents; checking running/ready/blocked work; recovering a stale local run; or designing a multi-stage repository task.
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

## Profile boundaries

| Profile | Use for | Can write/dispatch? | Preferred output |
|---|---|---:|---|
| `explore` / `scout` | Fast repository reconnaissance | No / No | Summary, evidence, architecture, start here |
| `plan` / `planner` | Concrete implementation planning | No / No | Goal, plan, files, tests, risks |
| `build` / `executor` | Implement and validate a scoped change | Yes / Yes | Completed, files changed, notes |
| `review` / `reviewer` | Correctness/security/test review | No / No | Findings, validation, recommendation |

Never broaden a read-only profile because it attempted a write or nested dispatch. Return the result to the parent.

## Output budgets for small models

Prompt limits are useful, but enforce output budgets in provider configuration when possible. The output budget includes reasoning, tool-call JSON, and the final answer.

For Qwen 4B or similar local models:

- `explore`: 8–12 bullets, roughly 1,500–2,048 output tokens;
- `plan`: 5–8 numbered steps and up to 5 risks, roughly 1,024–1,536 tokens;
- `review`: findings with severity and file/line evidence, roughly 1,500–2,048 tokens;
- `build`: allow more room for file bodies and commands; keep the final report under 20 lines.

Use explicit stopping instructions:

```text
Use tools only until the requested evidence is collected. Return only the requested sections. Limit the final answer to 500 words and 30 lines. Do not repeat tool output, speculate beyond evidence, or reveal private chain-of-thought.
```

Do not globally reduce the build budget to solve verbose planning; that can truncate tool calls or file contents.

## Validation checklist

- Confirm graph IDs are unique and dependencies are acyclic.
- Dispatch only ready nodes.
- Pass completed findings into dependent prompts.
- Check RunDeck for task ID, state, nested tool events, and result summary.
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
