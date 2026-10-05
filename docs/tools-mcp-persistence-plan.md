# RIGA tools, MCP, skills, and persistence plan

## Implemented in this slice

- Built-in catalog: `read`, `write`, `glob`, `grep`, `web`, `bash`, `shell`, `task`, and `skill`.
- WebSocket `tool_call` / `tool_result` protocol.
- Workspace-scoped path checks for file reads and writes.
- `RIGA_ENABLE_WRITES=1` gate for writes.
- `RIGA_ENABLE_SHELL=1` gate for bash/shell.
- HTTPS-only `web` tool with output truncation.
- Repository `skills/*/SKILL.md` discovery and loading.
- `.mcp.json` discovery and server cataloging without spawning arbitrary processes.
- Composer `+` menu inserts built-in tools, skills, and discovered MCP servers.
- AES-256-GCM encrypted local persistence for sessions and provider configuration.
- Encryption key is derived only from the runtime `RIGA_TOKEN`; no token is committed to source.
- `GET /health` reports `encrypted-file`, `sqlite`, or `postgres` based on `DATABASE_URL`.

## Persistence design

The persistence contract is deliberately adapter-neutral:

```text
SessionStore
  - list_sessions
  - create_session
  - append_event
  - replay_after_cursor
  - load_provider_config
  - save_provider_config
```

The encrypted-file adapter is the current browser-validation implementation. It stores atomic `.enc` files under `RIGA_DATA_DIR`, or `$HOME/.local/share/riga` by default. Ciphertext includes a random nonce and authenticated tag; plaintext provider keys and transcripts are never written.

The next database adapter should use the same contract and migrations:

- SQLite: `sessions`, `session_events`, `provider_config` tables, WAL mode, local file URL.
- Postgres: the same logical schema with `BIGINT` sequence and JSONB payloads.
- `DATABASE_URL=sqlite:...` and `DATABASE_URL=postgresql://...` select the adapter.
- `RIGA_TOKEN` remains the application-level encryption key for provider secrets even when the database is used.

## MCP design

The current catalog reads `.mcp.json` and exposes safe metadata to the UI. Process spawning is intentionally not enabled yet. The next MCP adapter will:

1. Parse a validated server command/args allowlist.
2. Spawn stdio MCP servers only in the Tauri/ trusted local runtime.
3. Perform initialize and `tools/list` JSON-RPC handshakes.
4. Expose namespaced tools as `mcp/<server>/<tool>`.
5. Route tool calls through the same approval and audit boundary as built-ins.

Browser mode must never execute arbitrary MCP commands on the host.

## Agent loop follow-up

The current WebSocket tool protocol can execute tool calls emitted by a trusted client and is ready for the provider loop. The next provider integration must add function/tool schemas to Chat Completions and Responses requests, execute returned calls through the registry, append tool results, and continue until a final answer or policy checkpoint. Tool calls must be denied by default when the relevant environment gate or approval is absent.
