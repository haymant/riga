

## Transcript and history follow-up

Streaming deltas are now coalesced into stable assistant, reasoning, and per-tool-output blocks before rendering. The TUI labels model output as `Assistant`, not `You`; user prompts are rendered separately. Reasoning is shown as a distinct `Thinking` block and tool activity remains distinct from assistant prose, with detailed tool state available in RunDeck.

The terminal now enables full keyboard escape-code reporting so compatible terminals preserve Shift+Enter. The TUI also accepts raw newline events as multiline composer input. Persisted conversation turns are loaded from the same server store when the TUI starts and whenever a history session is selected. GUI and TUI therefore share sessions and messages when `DATABASE_URL`, or alternatively `RIGA_DATA_DIR` plus `RIGA_TOKEN`, are identical.

Automated coverage now includes adjacent-delta coalescing, assistant-role rendering, multiline input, and the existing history and model-selection regressions. The CLI suite passes with **30 tests**; workspace clippy and tests pass.
