# Phase 5 Report

Phase: 5
Status: PASS

The server placeholder is now a runnable Axum HTTP/SSE adapter. It exposes `/health`, typed session listing and creation at `/sessions`, and a version-preserving `/runs/{run_id}/events` SSE stream. The adapter keeps transport concerns outside the kernel and serializes `RigaEventEnvelope` values without changing their domain shape. A runnable `riga-server` binary binds `RIGA_SERVER_ADDRESS` or defaults to `127.0.0.1:8787`.

The browser transport package now provides a typed `RigaHttpClient` with health, session, and streaming-event methods. Its SSE parser preserves event payloads and invokes a caller-owned callback, allowing the Phase 4 UI event source to be replaced without changing UI components.

Validation:

- `cargo fmt --all -- --check` — PASS
- `cargo clippy --workspace --all-targets -- -D warnings` — PASS
- `cargo test --workspace` — PASS (13 tests including server route coverage)
- `npm run check` — PASS
- Live HTTP health — PASS: `{"protocol_version":1,"adapter":"riga-server"}`
- Live session creation/listing — PASS
- Live SSE stream — PASS with ordered `riga.event` frames, event IDs, and versioned envelopes.

The initial resident-server attempt failed because its shell did not inherit the Rust toolchain PATH; the service was restarted with `$HOME/.cargo/env` loaded and the live verification then passed. This was an execution-environment issue, not an application failure.

Artifacts:

- `crates/riga-server/Cargo.toml`
- `crates/riga-server/src/lib.rs`
- `crates/riga-server/src/main.rs`
- `packages/transport-http/src/index.ts`
- `packages/transport-http/package.json`
- updated `Cargo.lock`

Known limitations: authentication, origin policy, persisted session-store wiring, command/run submission, and production SSE backpressure/reconnect semantics are still ahead in the security, policy, and release phases.

Next phase allowed: YES
