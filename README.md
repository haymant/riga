# RIGA

RIGA is a desktop-first coding-agent application built around a transport-neutral Rust kernel, Rig agent execution, a bidirectional WebSocket adapter, and a React/assistant-ui-inspired interface.

> **Status:** early development (`0.1.0`). APIs, persistence formats, and package names may change before the first stable release.

## Architecture

```text
React/Vite UI ── WebSocket ── riga-server ── riga-kernel ── Rig 0.43
      │                         │                 │
      ├─ composer + tools        ├─ HTTP/SSE      ├─ durable sessions
      ├─ attachments             ├─ encrypted     ├─ event journal
      └─ local preview           │  provider      └─ policy boundary
                                └─ workspace tools
```

The workspace is a **single Git repository and a multi-language monorepo**. It does not use Git submodules.

## Repository layout

- `crates/riga-kernel` — transport-free agent kernel, state, events, policy, and persistence abstractions.
- `crates/riga-server` — authenticated HTTP/WebSocket adapter, provider loop, tools, encrypted store, and temporary attachments.
- `crates/riga-cli` — command-line adapter boundary.
- `apps/riga` — thin Vite entry that mounts `@haymant/assistant-ui` inside a Tauri shell.
- `packages/assistant-ui` — the assistant chat surface (transcript, tool timeline, composer, settings, model manager) and its stylesheet.
- `packages/transport-http` — browser WebSocket transport client.
- `packages/transport-tauri` — Tauri transport boundary.

## Requirements

- Rust stable (the project currently validates with Rust 1.99.0)
- Node.js 22+
- npm 10+

## Local development

```bash
npm install
cp .env.example .env.local
npm run start
```

The preview is served on port `1420`. The kernel adapter listens on `8787` and the Vite server proxies `/health`, `/catalog`, `/sessions`, `/attachments`, and `/ws`.

Writes and shell commands are **approval-gated by the kernel's tool policy**, not by environment variables. The first time a run wants to write a file or run a shell command, it pauses and the UI shows an approval card with **Allow once**, **Always allow**, and **Decline**. "Always allow" is scoped to the session. Read-only tools run without asking; destructive tools are denied outright.

Never commit `.env.local`, provider keys, `RIGA_TOKEN`, or uploaded files. The API key is encrypted at rest by the server store and is write-only from the browser.

### Desktop shell

```bash
npm install
npm run tauri:dev
```

`tauri:dev` starts the kernel adapter and the Vite server through `beforeDevCommand`, then opens the native window against `http://127.0.0.1:1420`. Do not run `npm run dev` at the same time; port `1420` has `strictPort` enabled and both flows claim it. `npm run tauri:build` compiles the binary only, because `bundle.active` is `false` in `apps/riga/src-tauri/tauri.conf.json`.

Linux desktop builds need the WebKitGTK development packages (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`, plus the usual `build-essential` and `pkg-config`).

A bundled release binary is not yet wired: the shipped UI resolves `/ws` and `/catalog` relative to the `tauri://` origin, so the packaged app still needs the in-process server and the `packages/transport-tauri` boundary described in the roadmap.

## Provider configuration

Configure an OpenAI-compatible endpoint, model, reasoning effort, and API key in the UI. Provider metadata is restored from the encrypted server store; the key is never returned to the browser in restoration frames.

Set `RIGA_TOKEN` to the encryption secret used by the server-side store. Use a deployment secret manager in production.

## Tool and attachment model

Built-in tools include read, write, glob, grep, web, bash, task, and skill. Tool calls are rendered as collapsible call/result cards; bash and shell output use terminal-style blocks.

### Specialized agents

The `task` tool exposes four transport-neutral agent profiles:

| Agent | Aliases | Boundary | Purpose |
|---|---|---|---|
| `explore` | `scout`, `explorer` | Read-only | Reconnaissance and compressed codebase handoff |
| `plan` | `planner` | Read-only | Actionable implementation planning |
| `build` | `executor`, `worker` | Mutating, capability-gated | Implementation and repository validation |
| `review` | `reviewer` | Read-only | Independent correctness, security, and test review |

Use `task` with `action: "agents"` to discover profiles or `action: "dispatch"` with an `agent` and `prompt` to request a structured handoff. The parent RIGA run remains responsible for tool execution and approvals, so the profiles do not bypass workspace policy gates.

Attachments are uploaded through `POST /attachments` and stored under `tmp/riga-attachments/`. The generated workspace-relative path is included in the agent prompt so the read tool can inspect the file. Runtime attachments are ignored by Git.

## Validation

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check
npm test
npm run build --workspace @riga/desktop-ui
```

## Publishing

Rust crates are published from the workspace in dependency order:

```bash
cargo publish --dry-run -p riga-kernel
cargo publish --dry-run -p riga-server
cargo publish --dry-run -p riga-cli
```

The release workflow publishes crates on `v*` tags when `CARGO_REGISTRY_TOKEN` is configured. Public npm packages use the `@haymant` scope and publish through the release workflow when `NPM_TOKEN` is configured.

Create a release tag only after the validation workflow is green:

```bash
git tag -a v0.1.0 -m "RIGA v0.1.0"
git push origin v0.1.0
```

## Security

- Do not place secrets in source, fixtures, logs, transcripts, or Git history.
- Provider credentials are write-only from the browser and encrypted at rest by the server store.
- Workspace writes and shell execution are explicit capability gates.
- Web fetching is restricted to HTTPS.
- Review tool approvals before enabling mutation capabilities in shared deployments.

## License

MIT. See [LICENSE](LICENSE).
