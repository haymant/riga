# TUI Phase 0 Report

Status: **COMPLETE**

## Entry criteria

- IPC is the default TUI transport.
- WebSocket extraction is not a prerequisite.
- Existing `IpcService` methods are the runtime boundary.

## Exit criteria

- `riga-cli` has an IPC-backed headless entrypoint.
- Health, session listing/creation, catalog, active-run, approval, and event subscription operations are exposed through a CLI-owned adapter.
- Deterministic tests pass without an external server or provider.
- No API key is printed or persisted by the CLI.

## Files changed

- `crates/riga-cli/Cargo.toml`
- `crates/riga-cli/src/main.rs`
- `crates/riga-cli/src/transport.rs`
- `docs/BuildPlan-TUI.md`
- `docs/BuildPlan-TUI/TEST.md`
- `docs/BuildPlan-TUI/phase-0-report.md`
- `Cargo.lock`

The isolated `references/steer` checkout remains conceptual reference material only; no implementation code was copied into `riga-cli`.

## Tests run

All commands were run with the repository-compatible Rust toolchain loaded from `$HOME/.cargo/env`:

```text
cargo fmt --all
cargo test -p riga-cli
cargo test -p riga-server
cargo test --workspace
cargo clippy -p riga-cli --all-targets -- -D warnings
cargo run -p riga-cli -- --help
cargo run -p riga-cli -- --health
cargo run -p riga-cli -- --catalog
cargo run -p riga-cli -- --list-sessions
```

## Evidence

- `cargo fmt --all`: passed.
- `cargo test -p riga-cli`: **2 passed, 0 failed**.
- `cargo test -p riga-server`: **120 passed, 0 failed**.
- `cargo test --workspace`: **158 unit tests passed, 0 failed**; all doc-test targets passed with zero tests.
- `cargo clippy -p riga-cli --all-targets -- -D warnings`: passed with no warnings.
- `riga-cli --help`: passed and displayed the headless command surface.
- `riga-cli --health`: passed and returned the protocol health response.
- `riga-cli --catalog`: passed and returned the catalog JSON, including profiles, MCP servers, skills, and tools.
- `riga-cli --list-sessions`: passed and returned `[]` in the clean test workspace.
- IPC transport unit coverage verifies one shared in-process `ServerState` for default transport and safe empty active-run/approval operations.

## Known limitations

- Interactive Ratatui rendering is a later phase.
- WebSocket support is optional and not implemented in this phase.
- A provider-backed run is not exercised by CLI tests; server/kernel deterministic tests remain authoritative for agent execution.
- The CLI currently uses flags rather than interactive subcommands; this is intentional scaffolding for the TUI lifecycle introduced in later phases.

## Next phase

TUI-1 is complete; see `phase-1-report.md`. The next implementation phase is TUI-2:
terminal lifecycle, application event loop, Ratatui rendering, and idle/composer UI.
