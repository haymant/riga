# RIGA WebSocket Web-App Validation

## Answer

RIGA now runs as a Manus-previewable web app with one command:

```bash
npm run start
```

That command starts the Rust kernel adapter and the Vite web app together. Vite serves the React UI on port `1420`, proxies `/health`, `/sessions`, and `/ws` to the Rust server on port `8787`, and allows the Manus sandbox hostname. The public validation URL for this session is:

`https://1420-i9141dqdryhi087rg9m0a-3c741fda.sg2.manus.computer/`

The service is intentionally still running so it can be opened and tried in Manus.

## Transport design

SSE remains available at `/runs/{run_id}/events` and is appropriate for one-way server-to-browser event delivery. WebSocket is now the primary interactive browser transport because the coding-agent UI needs bidirectional control on the same connection: hello/readiness negotiation, run start, approval responses, cancellation, ping/pong, and streamed kernel events.

The WebSocket protocol is JSON-framed and version-preserving:

- Client commands: `hello`, `start_run`, `approval`, `cancel_run`, and `ping`.
- Server messages: `ready`, `event`, `approval_recorded`, `run_cancelled`, `pong`, and `error`.
- Kernel events remain `RigaEventEnvelope` values with protocol version, session ID, run ID, sequence, event ID, and domain event payload.

The current server uses a deterministic kernel bridge for validation. It emits `RunStarted`, two `TextDelta` events, and `RunCompleted` for a submitted prompt. Replacing that bridge with the real execution supervisor is isolated to the server/kernel integration and does not require changing the browser protocol or UI.

## Validation evidence

The public Manus browser successfully loaded the rendered RIGA UI, displayed `WebSocket connected`, and submitted a real prompt through the composer. The UI received and rendered the streamed kernel response.

A direct browser WebSocket smoke test against the public `wss://` endpoint completed with a `ready` message and four ordered events: `RunStarted`, two `TextDelta` events, and `RunCompleted`.

The local and public route manifests both return `GET /manus-routes.json` with the declared root route. The public health proxy returns protocol version `1` and adapter `riga-server`.

## Tests

Vitest contains nine focused tests covering the browser HTTP/SSE and WebSocket transports. Coverage is enforced through `vitest.config.ts` and passes the configured thresholds: 81.96% lines/statements, 92.68% branches, and 95.23% functions for the focused transport sources.

Rust validation passes formatting, Clippy with warnings denied, all kernel/server/desktop tests, and WebSocket protocol tests for valid start frames and unknown-command rejection. The production Vite build and strict TypeScript checks also pass.

## Known limitations

The Manus preview is a temporary sandbox service and will stop when its resident process is stopped or the sandbox expires. The public browser route is validated, but this is not yet a persistent hosted deployment. Authentication, reconnect/backoff policy, persisted approval state, and real provider-backed execution remain future hardening work.
