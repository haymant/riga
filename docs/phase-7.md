# Phase 7 — Unified Assistant SDK Transports and Tauri IPC Demo

## Handoff contract

This phase merges the **thin client protocol and transport adapters** into `@rigai/assistant-ui` while keeping all application behavior in `riga-server`.

The final dependency direction is:

```text
@rigai/assistant-ui
  ├── React components and styles
  ├── protocol types
  ├── http adapter (HTTP/WebSocket/SSE framing only)
  └── tauri adapter (invoke/events framing only)
          │
          └── riga-server application/runtime layer
                    │
                    └── riga-kernel
```

`riga-server` remains the only owner of provider execution, tools, approvals, MCP, sessions, persistence, attachments, and local GGUF management. No model, session, or policy logic may be added to TypeScript transports.

## Non-goals

- Do not move `riga-server` execution logic into npm.
- Do not create a second business-logic implementation for Tauri.
- Do not make the browser transport call Tauri APIs.
- Do not retain a required `file:../transport-http` dependency in the published assistant package.
- Do not silently fall back from Tauri IPC to HTTP in desktop mode.

## Subphase 7.0 — Baseline and package boundary

### Deliverables

- This handoff document is committed and pushed.
- Current unrelated worktree changes are preserved and not included accidentally.
- The intended package exports are recorded:
  - `@rigai/assistant-ui`
  - `@rigai/assistant-ui/protocol`
  - `@rigai/assistant-ui/http`
  - `@rigai/assistant-ui/tauri`
  - `@rigai/assistant-ui/styles.css`

### Exit criteria

```bash
git diff --check
git status --short
git show --check --stat HEAD
```

The commit must contain only the Phase 7 handoff document.

## Subphase 7.1 — Shared protocol and transport contract

### Deliverables

- Add a transport-neutral TypeScript contract under `packages/assistant-ui/src/protocol/`.
- Define shared event, session, provider, approval, catalog, attachment, and local-model types.
- Define `RigaTransport` with lifecycle, run-control, session, catalog, MCP, attachment, and local-model methods.
- Export the contract from `@rigai/assistant-ui/protocol` and the root package.
- Add compile-time tests or a typecheck fixture proving both adapters can implement the interface.

### Exit criteria

```bash
npm run typecheck --workspace @rigai/assistant-ui
npm run typecheck --workspace @riga/desktop-ui
npm test -- --run
```

No protocol type may be imported from `transport-http` or `transport-tauri`.

## Subphase 7.2 — HTTP adapter migration

### Deliverables

- Move/copy the current HTTP, WebSocket, SSE, and local-model framing clients into `packages/assistant-ui/src/http/`.
- Keep behavior compatible with the existing `riga-server` routes and WebSocket protocol.
- Ensure the HTTP adapter contains no provider, model, persistence, or policy logic.
- Export `createHttpTransport` and compatibility aliases from `@rigai/assistant-ui/http`.
- Remove the assistant package’s dependency on `@haymant/transport-http`.
- Keep `packages/transport-http` as a compatibility re-export only, or mark it deprecated.

### Exit criteria

```bash
npm run typecheck --workspace @rigai/assistant-ui
npm test -- --run
npm run check
```

The browser preview must complete the health handshake and receive a streamed run event from `riga-server`.

## Subphase 7.3 — Tauri IPC adapter contract

### Deliverables

- Implement `packages/assistant-ui/src/tauri/` using `@tauri-apps/api/core` `invoke` and Tauri event listeners.
- Provide the same `RigaTransport` operations as the HTTP adapter.
- Use explicit IPC command/event names; do not use `fetch`, `WebSocket`, `EventSource`, or loopback URLs.
- Implement reconnect/wake/close semantics for event subscriptions.
- Keep Tauri imports isolated to the `tauri` subpath so browser imports remain safe.

### Exit criteria

```bash
npm run typecheck --workspace @rigai/assistant-ui
npm test -- --run
```

A fake `invoke`/event harness must verify provider configuration, start/resume/cancel, approval, event delivery, and local-model event delivery.

## Subphase 7.4 — Thin `riga-server` IPC surface

### Deliverables

- Add a reusable server-side service/IPC command surface around the existing `ServerState` and provider runtime.
- Tauri commands must forward to the same server application methods used by HTTP/WebSocket routes.
- Implement command/event parity for:
  - health and catalog;
  - sessions and transcripts;
  - provider configuration;
  - run start/resume/cancel;
  - approval responses;
  - MCP registry and tool calls;
  - attachments;
  - local-model overview, actions, and progress.
- Emit durable run events through Tauri events with the existing `RigaEventEnvelope` shape.
- Do not duplicate tool execution or local-model logic in the Tauri crate.

### Exit criteria

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

A Rust IPC integration test must start a server state, invoke health/catalog/session operations, start a deterministic test run, observe ordered events, and cancel/resume without an HTTP listener.

## Subphase 7.5 — Assistant UI dependency injection

### Deliverables

- Change `AssistantUI` to receive a `RigaTransport` instance or factory.
- Remove direct imports of `@haymant/transport-http` from the component.
- Replace direct `fetch`, `WebSocket`, `EventSource`, and URL-building calls with transport methods.
- Preserve all existing UI behavior: sessions, streaming text, tool timeline, approvals, attachments, MCP connectors, local-model manager, model/reasoning selectors, themes, and composer triggers.
- Keep browser defaults using the HTTP adapter and allow demo to select the Tauri adapter explicitly.

### Exit criteria

```bash
npm run check
npm test -- --run
npm run build --workspace @riga/desktop-ui
```

A component test must render with a fake transport and verify that no browser network primitive is required.

## Subphase 7.6 — Demo application and documentation

### Deliverables

- Make `demo/` the reference React + Tauri consumer.
- Consume published `@rigai/assistant-ui` package exports where available; use explicit versioned dependencies, never a hidden workspace import.
- Use `@rigai/assistant-ui/tauri` in the desktop entrypoint.
- Use published `riga-kernel` and `riga-server` versions once the IPC surface is published; during development, document the exact temporary path dependency and replacement version.
- Remove HTTP proxy requirements from the Tauri path.
- Rewrite root `README.md` and `demo/README.md` with step-by-step browser and desktop IPC instructions.

### Exit criteria

```bash
npm install --prefix demo
npm run typecheck --prefix demo
npm run build --prefix demo
cargo check --manifest-path demo/src-tauri/Cargo.toml
```

The browser mode must continue using HTTP/WebSocket. The Tauri mode must use only `invoke` and Tauri events for assistant operations, with no `/ws`, `/health`, `/local-models`, or other HTTP requests from the webview.

## Subphase 7.7 — Release and handoff

### Deliverables

- Publish the corrected npm package version after all tests pass.
- Publish the corresponding Rust crate version containing the IPC service surface.
- Update CI and release workflow to validate the demo app.
- Add migration notes for consumers of `@haymant/transport-http` and `@haymant/transport-tauri`.
- Commit and push the final phase changes.

### Exit criteria

```bash
npm run check
npm test -- --run
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm install --prefix demo
npm run build --prefix demo
cargo check --manifest-path demo/src-tauri/Cargo.toml
git diff --check
git status --short
```

Every subphase must have its own commit and push before the next subphase begins. Never include secrets, generated model files, attachments, or unrelated worktree changes in a Phase 7 commit.
