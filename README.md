# RIGA

RIGA is a desktop-first coding-agent platform composed of a transport-free Rust kernel, a reusable assistant-ui React package, and thin Tauri, HTTP, and CLI adapters.

## Phase 0 bootstrap

```bash
rustup toolchain install 1.99.0 --profile minimal --component rustfmt clippy
npm install
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check
```

The implementation is phase-gated. See `docs/RIGABuildPlan.md` and the phase report under `docs/` before starting the next milestone.

## Boundaries

- `riga-kernel` must not depend on Tauri, React, browser APIs, or HTTP-server frameworks.
- Rig remains the model/tool execution runtime; RIGA owns product lifecycle, policy, persistence, events, and transports.
- Frontend packages never hold provider secrets or execute privileged commands.
