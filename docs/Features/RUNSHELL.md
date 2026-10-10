# Run shell

The run shell is everything that surrounds a model turn: the **workspace** a run
executes in, the **loop** that drives its turns, and the **native shell** helpers
that set both up. It is what keeps a run bounded, streamed, and recoverable.

## The workspace a run executes in

- The app gets a **deliberate, git-initialized workspace** (`ensure_workspace`)
  rather than inheriting its launch directory. A packaged app launches from
  `$HOME` and `tauri dev` from `src-tauri`; neither is a repository, so per-session
  worktrees would not engage and uploads would land in the watched source tree.
- The workspace is `git init`-ed with an empty initial commit, so `git worktree
  add` has a `HEAD` to fork from.
- Each session runs in its **own git worktree** under
  `<workspace>/.riga/worktrees/<session>`, so two sessions can create the same
  file without colliding, and a run's changes are isolated.
- Uploaded attachments live in the session worktree under
  `tmp/riga-attachments`, so the agent's tools find them at the relative path the
  prompt advertises.
- `RIGA_WORKSPACE_ROOT` points at the workspace; the data dir is
  `RIGA_DATA_DIR`, else `$HOME/.local/share/riga`, else `.riga-data`.

## Bundled skills

Bundled `skills/<name>/SKILL.md` files are seeded into
`<workspace>/skills/<name>/SKILL.md`, which is where the app reads them
(`catalog::load_skills`). Seeding is **refresh-aware**:

- a copy that still matches the hash we last wrote is refreshed when the bundled
  version changes;
- a copy the user has edited (hash differs) is preserved;
- a copy with no manifest entry is treated as an old bundled copy and refreshed.

The manifest is `<workspace>/.riga/skills-seed` (name, FNV-1a hash per line). This
replaced a "copy only if missing" seeder that could never ship a skill update to
an existing workspace.

## The run lifecycle

- A run emits `RunStarted`, then a stream of events, then exactly one terminal
  event: `RunCompleted { output }` or `RunFailed { message }`.
- Events are **journaled** per run and **broadcast** to subscribers; a client that
  reconnects replays from the journal after its last sequence. A slow subscriber
  that lags is told to catch up from the journal.
- The run registry tracks live runs so a second local run is refused with a clear
  message instead of blocking on the model mutex, and so **Active runs** can list
  and cancel them.
- Cancellation is a run-scoped flag threaded through the whole run, including
  subagents, so it reaches a model mid-decode and stops it at the next token.
- Both transports (Tauri IPC and WebSocket) carry the same events, so the
  desktop and web clients render identically.

## The local tool loop

One run is a loop of turns. Each turn formats the prompt, streams the reply,
parses tool calls, executes them, and decides whether to continue.

- **Token-aware prompt trimming** drops the oldest non-system messages until the
  prompt leaves room to answer, so a growing history cannot collapse the output
  budget to nothing.
- **Streaming** separates three things as they arrive: reply text (`TextDelta`),
  reasoning (`ReasoningDelta`), and tool lifecycle (`ToolCallStarted`,
  `ToolOutputDelta`, `ToolResult`). A tool-call block is kept out of the visible
  reply.
- **One call per reply** is the protocol; a turn that emits several tool calls is
  nudged back with a reminder, because batching is what drives re-planning loops.

### Loop guards

A weak local model loops. Each guard turns a specific loop into a clear stop:

| Guard | Constant | Limit | Stops when |
|---|---|---|---|
| Identical call | `MAX_REPEATED_TOOL_CALLS` | 2 repeats | the exact same call ran already |
| Same tool, varying args | `MAX_SAME_TOOL_CALLS` | 6 calls | one tool is looping with new arguments |
| Same tool set per turn | `MAX_REPEATED_TURNS` | 3 turns | the same *shape* of turn repeats |
| Consecutive failures | `MAX_CONSECUTIVE_TOOL_FAILURES` | 3 | the call cannot succeed as written |
| Whole run | `LOCAL_MAX_TOOL_CALLS` | 16 calls | the run is too broad |
| Turn budget | budget tier | — | the tier's turn count is reached |
| Wall clock | `LOCAL_RUN_WALL_CLOCK` | 600 s | the run has taken too long |

- A **disallowed** tool (one the profile lacks) is a redirect, not a failure, and
  does not count toward the consecutive-failure guard.
- **Watchdogs** distinguish a slow turn (per-turn deadline) from a stalled one
  (no-progress window), and report which fired.
- A **claim guard** annotates a final answer that claims work was done when no
  mutating tool succeeded.

### Reasoning dialect

A model's reasoning is surfaced through a per-model `ReasoningFormat { open,
close }`, resolved once at load:

- a curated entry may declare it; else it is read from the model's chat template;
  else the model has no reasoning channel and none is shown;
- `RIGA_LOCAL_REASONING=auto|on|off` forces the dialect on or off without a
  rebuild;
- the streamer matches the tags inline and holds back a partial opening tag, so a
  tag split across deltas is still caught.

Qwen3 emits its tags inline (its template opens nothing); Hermes 3, Phi-4 Mini,
and Qwen2.5 Coder have no reasoning channel, so they never show a block.

## Budget and context

- **Budget** — the run-scoped `LocalBudget` (`set_model_budget`) sets output,
  turns, and watchdog limits; tiers are `compact`/`middle`/`large`.
- **Context** — a fixed window, default 16k, up to `MAX_CONTEXT` (128k) via
  `RIGA_LOCAL_CONTEXT`. On a GPU build the KV cache is q8_0 with FlashAttention;
  the GPU plan is fitted with a 1.5 GiB margin and `n_ubatch` capped at 512, so a
  large window does not land the model on CPU with an enormous cache.

## Native shell

`riga-shell` runs before the Tauri runtime starts:

- `prepare_linux_display` sets conservative cursor defaults and prefers XWayland
  on Wayland, so GTK does not abort creating the first window.
- `default_data_dir` resolves the per-user data directory.
- `ensure_workspace` creates and `git init`s the workspace and seeds skills.

## Future work (ordered by least ROI)

1. **Configurable watchdog messages** — let a user tune the wording. Low: the
   messages are already clear and specific.
2. **A journal compaction pass** — trim old run journals. Low: journals are small
   and rarely read after a run.
3. **A workspace health check** — verify the git repo and worktree on start. Low:
   `ensure_workspace` is idempotent and already repairs the common case.
4. **Per-turn token accounting in the UI** — show tokens per turn. Moderate: the
   run budget is already enforced; this is diagnostic polish.
5. **Worktree cleanup** — prune stale session worktrees. Moderate: they are
   gitignored and isolated, but they do accumulate disk over time.
6. **A resume-after-crash path** — restart an interrupted run from its journal.
   Moderate-high: the journal has the events, but a model mid-decode cannot be
   resumed without re-generating the turn.
7. **Persistent context across turns** — keep the `LlamaContext`/KV cache alive
   so only new tokens are decoded. High: the biggest local-latency win, but the
   binding makes `LlamaContext` `!Send` and borrow the model, so it needs an
   inference-thread actor owning the model and context.
8. **A deferral guard** — detect a final answer that leaves the work to the user
   ("run this yourself") and treat it like the claim guard treats an unsupported
   claim. High: it is the failure mode that let a round "complete" without
   producing the requested result.
