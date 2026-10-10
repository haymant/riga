# TUI Phase 2 Report

Status: **COMPLETE**

## Scope

TUI-2 adds the first interactive Ratatui surface while keeping transport and projection code separate. The explicit `--tui` mode owns terminal setup and teardown, renders a responsive idle/transcript/composer/footer layout, and uses the IPC-backed application state as its initial connection state.

## Files changed

- `crates/riga-cli/Cargo.toml`
- `Cargo.lock`
- `crates/riga-cli/src/main.rs`
- `crates/riga-cli/src/app.rs`
- `crates/riga-cli/src/input.rs`
- `crates/riga-cli/src/terminal.rs`
- `crates/riga-cli/src/ui.rs`
- `docs/BuildPlan-TUI/phase-2-report.md`

## Implemented behavior

`TextBuffer` edits Unicode and multiline input using character boundaries rather than arbitrary byte slicing. Enter returns one submit action and clears the draft; Shift+Enter inserts a newline. Backspace and left/right movement are deterministic, and unknown keys are ignored.

`ui::render` is pure with respect to `UiState` and terminal dimensions. It renders connection state, session, transcript entries with separate reasoning/tool/task/approval styles, a bounded composer, and a footer status. The layout has TestBackend coverage at 80×24 and 120×40, including disconnected-state visibility.

`TerminalGuard` owns raw mode, alternate-screen entry, drawing, and Drop-based cleanup. `--tui` is explicit in this phase so existing machine-readable health/catalog/session commands remain deterministic in non-interactive environments.

## Validation evidence

Executed with the repository Rust toolchain loaded from `$HOME/.cargo/env`:

```text
cargo fmt --all
cargo test -p riga-cli
cargo clippy -p riga-cli --all-targets -- -D warnings
cargo test --workspace
git diff --check
```

Results:

- Formatting passed.
- `cargo test -p riga-cli`: **12 passed, 0 failed**.
- `cargo clippy -p riga-cli --all-targets -- -D warnings`: passed with no warnings.
- `cargo test --workspace`: **169 unit tests passed, 0 failed**; all doc-test targets passed with zero tests.
- `git diff --check`: passed.

## Manual terminal smoke

Run `cargo run -p riga-cli -- --tui` from an interactive terminal. Verify that the alternate screen is entered, `Shift+Enter` creates a multiline draft, Enter clears the draft, `Esc` exits, and the shell is restored after exit. Automated TestBackend coverage verifies layout and the application key seam without requiring a real terminal.

## Next phase

TUI-3: streamed transcript cards, approval interaction, follow-output scrolling, and transport command dispatch for real runs.
