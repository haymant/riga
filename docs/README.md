# RIGA docs

## Development management

`docs/BuildPan-*/` (the "BuildPlan" series) holds **RIGA development
management** — the working material that drives the project: build plans,
architecture and requirements plans, phase reports, and implementation
handoffs.

- `docs/BuildPan-1/` — the first plan series (original build plan, phase 0–7
  reports, product architecture and requirements).
- `docs/BuildPan-2/` — the current plan series (graph-native runtime evolution).

These are living documents. They change as the plan changes.

## Features

`docs/Features/` holds the **final knowledge snapshot of RIGA features** — the
settled description of what RIGA actually does. A feature lands here once it is
complete, so this directory is the durable, current record of RIGA's
capabilities, kept separate from the evolving build plans above.

- `SUBAGENT.md` — orchestration: profiles, dispatch, graph nodes, tool grants,
  the model budget, and the read-only boundaries.
- `EVIDENCE.md` — the automatic record of what ran, and the Evidence lens.
- `KNOWLEDGE.md` — the reusable lessons a run leaves, and their injection into
  the next run.
- `RUNDECK.md` — the command center: the panel above the composer, its lenses
  (execution, evidence, knowledge), the subagent queue, and the task graph.

Each doc ends with **potential future work ordered by least ROI**.
