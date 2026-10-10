# RunDeck

RunDeck is RIGA's **command center**: the panel above the composer where a run is
monitored and controlled. It shows the subagent queue, the evidence and knowledge
a run collects, and — when the run publishes one — an interactive task graph.

Where `SUBAGENT.md`, `EVIDENCE.md`, and `KNOWLEDGE.md` describe the runtime, this
doc describes the surface the user watches and steers. The model-facing side is
the `rundeck` skill (`skills/rundeck/SKILL.md`); RunDeck is what that skill's
orchestration looks like in the product.

## What it is

RunDeck is a single collapsible panel with three lenses and a set of live cards.
It answers four questions at a glance:

- **What is running?** — the header's progress and the Active runs list.
- **What is the plan?** — the plan and todo cards.
- **Who is doing what?** — the subagent queue, each child with its own tool calls.
- **What have we learned?** — the Evidence and Knowledge lenses.

It is a **view over the run's event stream**, not a separate store: every card is
driven by the same `RigaEvent`s the run emits (plan/todo/graph updates, task
lifecycle, evidence, knowledge), so it is identical in the desktop and web
clients.

## Where it lives

- **Expanded**, it sits directly above the composer.
- **Collapsed**, it is a one-line summary in the composer footer; clicking it
  expands the deck.

It renders only when there is something to show — a running run, an active run,
or any plan, todo, task, graph, evidence, or knowledge state. Otherwise it is
absent, so an idle session has no empty chrome.

## The header

The header is the always-visible summary:

- **Title** — "Run Deck", the current lens, and the selected run id.
- **Progress** — `done/total`, where `total` is the graph's node count (else the
  task count) and `done` counts settled subagents (completed or failed).
- **Live state** — a "thinking" pulse while the run is in flight, and `running
  <agent>` naming the child that is executing.
- **Graph readiness** — `N ready · M blocked` when a graph exists.
- **Collapse toggle.**

## Active runs and cancellation

When the app knows of live runs (after connecting or reconnecting), RunDeck lists
them:

- each row shows the **run id**, its **session**, and whether it is **local**;
- a **Stop** button cancels that specific run;
- a **Stop active run** control cancels the current run and its child agents.

This is the recovery surface for `local_run_in_progress`: rather than starting a
competing local run, the user opens RunDeck, inspects the active run, and stops it
or waits.

## The lenses

A tab strip switches the deck's main view:

### Execution

The subagent **queue** and, when the run published a graph, the **task graph**.
This is the default lens: it shows who is running, ready, blocked, completed, or
failed.

### Evidence

The **Evidence** cards: every outcome the harness recorded — failures, successful
mutating calls, and subagent tasks — each with its claim, source reference, and
confidence. Reads and globs are logged for the run summary but not surfaced, so
the lens keeps its signal. (See `EVIDENCE.md`.)

### Knowledge

The **Knowledge** cards: the reusable lessons a run derived — repeated failures,
individual failures, and what each subagent achieved — each with its confidence
and source run. These persist per session and are injected into the next run.
(See `KNOWLEDGE.md`.)

## The cards

Below the lens, three cards are always shown when they have content:

- **Plan** — the orchestrator's current plan (`update_plan`), with the active
  step marked.
- **Todos** — the working checklist (`update_todos`).
- **Subagents** — the queue of dispatched children. Each row shows the profile,
  the description, a state icon (`•` running, `✓` done, `✕` failed, `!` blocked,
  `○` ready), an optional progress bar, its blocking dependencies, and the first
  line of its result. A child with nested tool calls is **collapsible**: expanding
  it lists that child's tool calls with their status, so a subagent's work is
  inspectable without leaving the deck.

The subagent card is the answer to "who is doing what": the parent's tool calls
and each child's are attributed to the right row by task id.

## The execution graph

When a run publishes a graph (`update_graph`), the Execution lens renders it with
**xyflow** (`@xyflow/react`):

- **Layout** is by dependency depth — a node's column is its longest dependency
  chain, so the graph reads left-to-right as a DAG.
- **Edges** are drawn dependency → dependent; an edge is animated while its target
  is running, and styled *blocked* or *satisfied*.
- **Node color/state** — ready, running, waiting for approval, blocked, completed,
  failed.
- **Navigation** — pan/zoom, a MiniMap, and Controls. Clicking a node **focuses**
  it: a breadcrumb (`Session › Run › <node>`) appears and the graph narrows to the
  node and its dependents.
- **Accessibility fallback** — a plain dependency list sits under the canvas, so
  the graph is still usable by keyboard and without the canvas.

## Run scope

A **Run scope** picker selects which run the deck is showing — the active run, or
an earlier one in the session. The selected run drives the graph, cards, and
lenses, so the deck can inspect a finished run as well as the live one.

## RunDeck and the skill

The two are deliberately separate:

- **RunDeck** is the **UI** — the monitor and control surface.
- **`skills/rundeck/SKILL.md`** is the **model-facing protocol** — when to
  dispatch, how to form a graph, how to read the statuses, and how to recover a
  failed run.

A user who wants a run to be orchestrated well should keep the skill in play: the
skill is what makes the model publish a graph, pass findings between children,
and respect the read-only boundaries that RunDeck then visualises. RunDeck shows
the result; the skill shapes it.

## Future work (ordered by least ROI)

1. **Collapse state memory** — remember the expanded/collapsed choice per session.
   Low: it is one click, and the current default is sensible.
2. **Node tooltips** — show a node's full prompt on hover. Low: the description
   and the focused breadcrumb already carry the intent.
3. **A legend** — spell out the state colors. Low: the icons and labels are
   already textual.
4. **Timeline view** — a per-node elapsed-time track. Moderate: `TaskStatus`
   already carries `elapsed_ms`, but the graph is about readiness, not duration.
5. **Deep-linkable focus** — put the focused node in the URL/session so a view can
   be shared or restored. Moderate: useful for review, but the focus is cheap to
   re-select.
6. **Evidence filtered to the focused node** — when a graph node is focused, scope
   the Evidence lens to that node's task. Moderate-high: the data is already
   tagged with `task_id`; it turns the deck from a run view into a per-child view.
7. **Inline approval controls** — approve/deny an approval-gated call from the
   deck instead of the transcript. High: approvals already flow through a broker,
   and surfacing them where the run is monitored removes a context switch.
8. **Re-run a failed node** — a retry button on a failed graph node, passing the
   recorded evidence and knowledge into the new dispatch. High: it closes the
   loop the knowledge system was built for — fail, learn, retry — directly from
   the command center.
