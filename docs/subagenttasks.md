# RIGA subagent and task harness

This document is the validation guide for RIGA's **live subagent dispatch**, **durable task records**, and **AssistantUI task rendering**. It reflects the current implementation after the IPC and assistant-ui transport work.

## 1. Current architecture

RIGA has two related task concepts:

1. **Live subagent tasks** belong to one agent run. The root run is the orchestrator; the `task` tool dispatches child agents. The server emits task lifecycle events through the normal `RigaEventEnvelope` stream, and `AssistantUI` renders them in the **Subagents** card.
2. **Durable task records** are cross-run bookkeeping records managed by the `task` tool. They are separate from live subagent execution and use the configured secure persistence store.

The relevant implementation boundaries are:

| Area | Source of truth | Responsibility |
|---|---|---|
| Task tree and limits | `crates/riga-kernel/src/task.rs` | `TaskRecord`, lifecycle states, parent links, depth and fan-out limits |
| Wire events | `crates/riga-kernel/src/events.rs` | `TaskStarted`, `TaskStatus`, `TaskCompleted`, approval events, and ordered envelopes |
| Profiles and direct task actions | `crates/riga-server/src/catalog.rs` | Profiles, aliases, `agents`, `dispatch`, and durable `list/inspect/create/update` actions |
| Live dispatch and execution | `crates/riga-server/src/ws.rs` | Dispatch interception, concurrent sibling execution, restricted tools, task-scoped tool events, and journaling |
| In-process desktop service | `crates/riga-server/src/ipc.rs` | Tauri-facing server API; delegates to the same provider/run pipeline rather than duplicating it |
| UI rendering | `packages/assistant-ui/src/AssistantUI.tsx` | Subagent cards, completion counts, nested task tool calls, and result summaries |
| UI coverage | `packages/assistant-ui/src/AssistantUI.test.tsx` | Fake transport tests for running/completed/failed subagents and nested tool output |

## 2. Supported subagent profiles

The canonical profiles and aliases are defined in `crates/riga-server/src/catalog.rs`:

| Profile | Aliases | Mutates files/system? | Intended use | Tools |
|---|---|---:|---|---|
| `explore` | `scout`, `explorer` | No | Fast, read-only codebase reconnaissance | `read`, `glob`, `grep`, read-only `bash` |
| `plan` | `planner` | No | Actionable implementation planning | `read`, `glob`, `grep`, read-only `bash` |
| `build` | `executor`, `worker` | Yes | Implement a scoped change and validate it | `read`, `write`, `glob`, `grep`, `bash`, `task`, `skill` |
| `review` | `reviewer` | No | Review correctness, security, tests, and maintainability | `read`, `glob`, `grep`, read-only `bash` |

Read-only profiles cannot write files, change system state, or dispatch nested agents. The `build` profile can modify the workspace, but it still obeys RIGA's approval and capability gates.

Every profile has a model preference, system rules, and an expected output format. The current expected sections are:

- `explore`: Summary, Answer, Files Retrieved, Key Code, Architecture, Start Here
- `plan`: Goal, Plan, Files to Modify, New Files, Risks / Assumptions
- `build`: Completed, Files Changed, Notes
- `review`: Summary, Findings, Validation, Recommendation

## 3. Task tool actions

The `task` tool has explicit action routing:

| Action | Behavior | Starts a live subagent? |
|---|---|---:|
| `agents` or `agent_list` | List compact profile metadata and the exact dispatch form | No |
| `dispatch` | Run a named profile with a prompt | Yes, when intercepted by the run loop |
| `agent` or `run` | Accepted aliases for `dispatch` | Yes |
| `list` | List durable task records | No |
| `inspect` | Inspect one durable task by `task_id` | No |
| `create` | Create a durable task record | No |
| `update` | Update a durable task record | No |

A direct dispatch requires `agent` (or `name`) and a non-empty `prompt`. The preferred explicit shape is:

```json
{
  "action": "dispatch",
  "agent": "explore",
  "prompt": "Inspect the task implementation and summarize the execution flow.",
  "description": "Inspect task execution"
}
```

Aliases such as `scout`, `planner`, `executor`, and `reviewer` resolve to their canonical profiles. Listing profiles is informational only; it must not be reported as completed work and must not create a Subagents progress card.

## 4. Live task lifecycle and limits

A live task is represented by `TaskRecord` and may be in one of these states:

- `pending`
- `running`
- `waiting_for_approval`
- `completed`
- `failed`
- `cancelled`

Terminal states are immutable. A late result cannot resurrect a cancelled task, which keeps event replay safe.

Current limits in `crates/riga-kernel/src/task.rs`:

- **Maximum nesting depth:** `2` levels below the root task.
- **Maximum child fan-out:** `4` children per parent.
- **Sibling dispatch:** multiple dispatch calls in one model response run concurrently, so the UI can show parallel workers.

The live event sequence uses the shared `RigaEventEnvelope` shape and includes:

- `TaskStarted { task }`
- `TaskStatus { task_id, state, elapsed_ms }`
- `TaskCompleted { task_id, ok, result }`
- `ApprovalRequested { approval_id, task_id, tool, summary }`
- `ApprovalResolved { approval_id, approved, reason }`

Subagent tool calls include `task_id`. `AssistantUI` uses that field to nest read/write/shell/tool activity under the correct subagent card instead of placing it in the root timeline.

## 5. Durable task records

Durable records are separate from live dispatch. The supported lifecycle is:

```json
{
  "action": "create",
  "title": "Test durable task",
  "description": "Verify task creation, inspection, update, and listing.",
  "status": "open"
}
```

Then use the returned ID:

```json
{ "action": "inspect", "task_id": "<TASK_ID>" }
```

```json
{
  "action": "update",
  "task_id": "<TASK_ID>",
  "status": "completed",
  "description": "The durable task lifecycle was verified successfully."
}
```

Finally:

```json
{ "action": "list" }
```

Durable records require the session's configured secure persistence store. If it is unavailable, the tool returns a neutral message directing the caller to `update_plan` and `update_todos` for in-run progress. Do not treat that fallback as a live-agent failure.

## 6. Manual validation prompts

Run these prompts in the RIGA chat UI. Use a fresh session when validating event ordering.

### 6.1 Direct profile listing

```text
Use the task tool exactly once with action "agents" to list the available subagent profiles. Do not dispatch an agent. Summarize the returned profiles.
```

Expected behavior:

- The tool returns the compact profile list.
- No Subagents progress card appears.
- No child agent starts.

### 6.2 Deterministic direct dispatch

```text
@explore Inspect the implementation of the task tool and subagent dispatch in this repository. Explain the execution flow, the available profiles, the tool restrictions, and the relevant Rust files. Do not modify anything.
```

Expected behavior:

- A **Subagents** card appears.
- It first shows `explore` as running and `0/1` complete.
- Read-only calls appear nested under the worker.
- The card ends at `1/1` and shows the returned summary.
- No files are modified.

The equivalent explicit tool request is:

```json
{
  "action": "dispatch",
  "agent": "explore",
  "prompt": "Inspect the repository provider configuration and explain how settings are persisted. Do not modify files.",
  "description": "Inspect provider persistence"
}
```

### 6.3 Planning profile

```text
@plan Create an actionable plan for adding a task-history panel to the assistant UI. Inspect the existing session, task, and AssistantUI implementations. Do not edit files. Include the exact files and functions that would need modification.
```

Expected result sections: **Goal**, **Plan**, **Files to Modify**, **New Files**, and **Risks / Assumptions**.

### 6.4 Review profile

```text
@review Review the current subagent and task implementation for correctness, security boundaries, cancellation behavior, nesting limits, and test coverage. Do not modify files. Report findings with severity and file references.
```

Expected result sections: **Summary**, **Findings**, **Validation**, and **Recommendation**.

### 6.5 Build profile

Use only a harmless, narrowly scoped change in a disposable branch or workspace:

```text
@build Add a short explanatory comment above the AgentTaskList component in packages/assistant-ui/src/AssistantUI.tsx, then run the smallest relevant validation command. Report the file changed and the validation result.
```

Expected behavior:

- The build profile may write and run shell commands.
- Approval gates still apply.
- The result identifies the changed file and validation command.
- Revert the comment after the harness check if it is not a desired product change.

### 6.6 Durable task lifecycle

Use the four actions in order: `create`, `inspect`, `update`, and `list`. Verify that the returned record has an ID, status, description, and timestamps, and that the updated record is visible in the final list.

### 6.7 Parallel sibling dispatch

```text
Inspect this repository by dispatching two independent read-only subagents in parallel:
1. An explore agent to inspect the Rust task tree and lifecycle limits.
2. A review agent to inspect the AssistantUI subagent rendering and event handling.

Do not modify files. Combine their findings into one comparison.
```

Expected behavior:

- Two sibling workers run concurrently.
- Both appear under one Subagents card.
- Completion count advances as each worker settles.
- Each worker has its own nested tool calls and result summary.
- The combined answer compares both outputs.

## 7. UI assertions

For every real dispatch, verify the AssistantUI shows:

- **Subagents** heading and an accessible region named `Agent subagents`.
- Completion count such as `0/1`, `1/1`, or `1/2`.
- Canonical agent name and description.
- Running, completed, or failed state.
- Expandable/nested tool activity associated with `task_id`.
- The first line or summary of the subagent result.
- A failure count when one or more workers fail, for example `1/1 · 1 failed`.

The current component tests cover:

- A running `explore` task.
- A nested `read` tool call with `task_id`.
- Incremental `ToolOutputDelta` and final `ToolResult` rendering.
- A completed result and `1/1` count.
- A failed `review` task and its failure summary.

## 8. Automated validation

Run the focused frontend tests:

```bash
npm test -- --run packages/assistant-ui/src/AssistantUI.test.tsx
```

Run all TypeScript checks and tests:

```bash
npm run check
npm test
```

Run the Rust task and IPC coverage:

```bash
cargo test -p riga-kernel task
cargo test -p riga-server task
cargo test -p riga-server ipc
cargo test --workspace
```

The Rust coverage should include:

- Task tree parent links and depth calculation.
- Maximum depth and four-child fan-out rejection.
- Terminal-state immutability and cancellation winning over late completion.
- Profile alias resolution and read-only tool restrictions.
- Explicit dispatch validation and `agents` list-only behavior.
- Durable task fallback when secure persistence is unavailable.
- Event serialization for task, approval, plan, and todo events.
- IPC health/catalog/session operations and the deterministic run/event path.

## 9. Troubleshooting

- **The task tool lists profiles but no card appears:** `action: "agents"` is list-only. Use `action: "dispatch"` with `agent` and `prompt` for a live worker.
- **The model says it dispatched but no worker started:** use the explicit dispatch shape; do not rely on an unstructured `task` call.
- **A read-only agent attempts a write:** inspect the profile's allowed tools and report the violation; do not broaden the profile as a workaround.
- **A task says durable storage is unavailable:** use `update_plan`/`update_todos` for run-local progress, or configure the session's secure store before testing durable records.
- **Nested tool output appears in the root timeline:** verify the emitted tool call includes the correct `task_id` and that the UI received `TaskStarted` before the tool event.
- **A late result changes a cancelled worker:** this is a server bug; `TaskTree::settle` must preserve `cancelled`.
- **Parallel dispatch exceeds the limit:** the current maximum is four direct children per parent and two nested levels.

Never include API keys, `RIGA_TOKEN`, provider secrets, generated model files, or uploaded attachments in this guide or in test prompts.
