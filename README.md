# RIGA

RIGA is a desktop-first coding-agent application built around a transport-neutral Rust kernel, Rig agent execution, and a reusable `@rigai/assistant-ui` surface. Browser consumers use HTTP/WebSocket framing; desktop consumers use in-process Tauri IPC commands and events.

> **Status:** early development (`0.1.0`). APIs, persistence formats, and package names may change before the first stable release.

## Architecture

```text
@rigai/assistant-ui ── HTTP/WebSocket ──┐
                  └── Tauri IPC ────────┼── riga-server ── riga-kernel ── Rig 0.43
                                       │       │                 │
                                       │       ├─ provider/tools  ├─ durable sessions
                                       │       ├─ approvals/MCP   ├─ event journal
                                       │       └─ persistence     └─ policy boundary
```

The workspace is a **single Git repository and a multi-language monorepo**. It does not use Git submodules.

## Repository layout

- `crates/riga-kernel` — transport-free agent kernel, state, events, policy, and persistence abstractions.
- `crates/riga-server` — authenticated HTTP/WebSocket adapter, provider loop, tools, encrypted store, and temporary attachments.
- `crates/riga-cli` — command-line adapter boundary.
- `demo` — reference React + Tauri consumer and browser development app.
- `packages/assistant-ui` — the assistant surface plus `protocol`, `http`, and `tauri` transport subpaths.
- `crates/riga-server/src/ipc.rs` — reusable in-process service used by the Tauri host.

The **desktop** (Tauri) client talks to the in-process server over **IPC**; the
**browser** client talks to the standalone `riga-server` over **HTTP/WebSocket**.
Both implement one `RigaTransport` interface (`packages/assistant-ui/src/protocol`),
and a conformance test asserts they stay in step.

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

The local GGUF context window is capped at `32768` tokens (the KV cache is sized from it). On a machine with limited memory headroom, set `RIGA_LOCAL_CONTEXT` to a smaller value — for example `8192` — to shrink the KV cache; the GGUF's own trained window still clamps the result.

### Start the CLI

The CLI uses the CPU backend by default. From the repository root, start the
interactive terminal UI with:

```bash
cargo run -p riga-cli -- --tui
```

The non-interactive health, session, and catalog commands use the same CPU
default. For example:

```bash
cargo run -p riga-cli -- --health
cargo run -p riga-cli -- --list-sessions
cargo run -p riga-cli -- --catalog
```

To build and start the CLI with NVIDIA CUDA offload, use the repository wrapper.
It provides the server CUDA feature and prepares a compatible CUDA toolkit,
stale-CMake recovery, and the glibc/CUDA header compatibility shim required by
CUDA 12.6 on newer Ubuntu systems:

```bash
npm run cli:cuda -- --tui
```

Set the GPU architecture and toolkit explicitly when needed:

```bash
CMAKE_CUDA_ARCHITECTURES=89 CUDA_PATH=/usr/local/cuda-12.6 npm run cli:cuda -- --tui
```

The CUDA build requires a compatible NVIDIA driver, the NVIDIA CUDA Toolkit
(including `nvcc`), `cmake`, `ninja-build`, and a C++ toolchain (`build-essential`
on Debian/Ubuntu). The wrapper detects the glibc/CUDA `cospi`, `sinpi`, and
`rsqrt` `noexcept` conflict shown by CUDA 12.6 and creates a private patched
header include tree; it never edits the root-owned CUDA installation.

llama.cpp/ggml and the CUDA runtime (cuBLAS, cuBLASLt, cudart) are linked
**dynamically**, so the executable stays small and the shared llama.cpp libraries
are staged next to it. A machine that *runs* a CUDA build therefore needs both
the NVIDIA driver and the CUDA runtime libraries on its loader path; installing
the CUDA Toolkit configures this via `ld.so.conf`.

To run the **published** CLI (from crates.io) instead of this checkout, use the
published wrapper. It installs `riga-cli` with the `cuda` feature, stages the
llama.cpp/ggml shared libraries next to the installed binary (so its `$ORIGIN`
rpath resolves them), then runs it:

```bash
npm run cli:cuda:published -- --tui
```

It installs to `~/.local/bin` by default (`RIGA_CLI_PREFIX` overrides it).
Before the crate is published, exercise the same path against the local sources:

```bash
RIGA_CLI_FROM_PATH=1 npm run cli:cuda:published -- --health
```

The CPU CLI needs none of this — `cargo install riga-cli --version 0.1.14` is
self-contained, because only the CUDA build links dynamically.

CUDA is a compile-time backend choice; merely running the CPU binary on a
CUDA-capable machine does not enable GPU offload. If a direct Cargo build was
interrupted and reports `CMAKE = None` or `Makefile: No such file`, first run:

```bash
cargo clean -p llama-cpp-sys-2
```

Then use `npm run cli:cuda -- --tui` rather than the direct Cargo command. The
server still auto-fits local-model layers to currently available GPU memory.
If no CUDA toolkit is installed, use the CPU command above.

### Selecting a model in the CLI

Start the TUI with `cargo run -p riga-cli -- --tui`. From the transcript:

1. Press **`Ctrl+,`** to open **Provider settings**.
2. The **model** row is the third field. Use **Tab** or **Shift+Tab** to reach it, type the model identifier, then press **Ctrl+S** to save.
3. For a local GGUF model, press **`Ctrl+L`** to open **Local models**. Select an installed model and press **Enter** to load it, or select a catalog entry and press **Enter**/**`d`** to download it. Loading a model also selects its model id for the next run.
4. Press **`?`** at any time for the keyboard help panel. Press **Esc** to close a panel.

The local-model panel also shows the active accelerator, installed/catalog models,
download cancellation (`x`), unload (`u`), and attachment upload (`a`). Attachment
upload accepts a local file path and stores the file in the selected session's
worktree through the shared IPC service.

`Ctrl+H` (or Backspace when the empty composer receives a terminal's Ctrl+H
encoding) opens the same persisted session list used by the GUI. Selecting a
session now reloads its persisted user and assistant messages into the TUI.
Both clients must use the same persistence configuration: `DATABASE_URL` if a
database is configured, or the same `RIGA_DATA_DIR` and `RIGA_TOKEN` otherwise.
Without `RIGA_DATA_DIR`, the default data directory is
`~/.local/share/riga`. A GUI started with a different token, database URL, or
home directory will intentionally see a different store. `Ctrl+,` and **F2**
both open Provider settings for terminals that do not reliably transmit
Ctrl-comma.

Never commit `.env.local`, provider keys, `RIGA_TOKEN`, or uploaded files. The API key is encrypted at rest by the server store and is write-only from the browser.

### Reference demo and desktop shell

```bash
npm install --prefix demo
npm run dev --prefix demo       # browser HTTP/WebSocket mode
npm run tauri dev --prefix demo # desktop Tauri IPC mode
```

`demo/` is the reference consumer. Browser mode starts `riga-server` on `127.0.0.1:8787` and uses the Vite proxy. Tauri mode starts `riga-server` in-process and passes `createTauriTransport` to `AssistantUI`; assistant operations use only `invoke` and Tauri events, with no HTTP loopback fallback. See [`demo/README.md`](demo/README.md) for the complete step-by-step integration and provider/model checks.

The header also opens a **file explorer** and a **source control** panel. Both are two-pane and resizable, with the navigation list on the left at about a third of the width and the content on the right. The explorer lists the session's worktree lazily: only the first level is read, and expanding a directory asks the backend for that one level; selecting a file previews it (markdown, or a line-numbered code view for JS/TS, Python, Rust, JSON, YAML, and more). The source control panel splits its left column into **Changes** (staged/unstaged files, with the commit message and Commit/Push buttons) on top and **History** (recent commits) below; selecting a changed file shows its working-tree diff and selecting a commit shows that commit's diff. `riga-server` owns this through `GET /workspace/entries`, `GET /workspace/file`, `GET /git/status`, `GET /git/diff`, `GET /git/log`, `GET /git/commit-diff`, `POST /git/commit`, and `POST /git/push`; the Tauri transport mirrors them as IPC commands. Every path is resolved under the session worktree and rejected if it escapes it.

Linux desktop builds need the WebKitGTK development packages (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`, plus the usual `build-essential` and `pkg-config`).

The packaged build also uses the Tauri IPC path. Its webview does not need a server URL, HTTP proxy, browser WebSocket, or EventSource permission for assistant operations. `riga-server` remains the single application/runtime owner behind both transport adapters.

Every native shell calls `riga_shell::prepare_linux_display()` as the first statement in `main`, before the Tauri/GTK runtime starts. It lives in `crates/riga-shell` because it is a native-shell concern: it sets conservative cursor defaults and prefers XWayland when a `DISPLAY` is present, avoiding a Wayland cursor-theme assertion that otherwise aborts the app before the window appears. It deliberately is not in `riga-kernel` or `riga-server`, which also run headless (CLI, tests, HTTP), so a display side effect there would be wrong.

### GPU builds

The local GGUF runtime links llama.cpp, which can offload to an NVIDIA GPU
(`cuda`) or Apple Silicon (`metal`). The release workflow builds Linux and
Windows on CPU, macOS with Metal, and Linux with CUDA on demand: open the
**Release RIGA desktop app** workflow and run it manually with **Build
linux-cuda only** checked (a manual run builds only that variant; the CPU and
macOS jobs run on tag pushes). At load time the model's GPU plan is auto-fitted
to the memory actually free, so it does not blindly offload every layer.

The demo ships a wrapper that runs Tauri with the `cuda` feature and does the
build setup for you — a coherent CUDA toolkit, position-independent CUDA objects
for the cdylib link, and (on glibc ≥ 2.41) a header shim for the `rsqrt`
exception-specification clash. Because the CUDA build links dynamically, the
llama.cpp shared libraries are copied next to the executable (`$ORIGIN`) so it
runs straight from `target/`:

```bash
npm run tauri:dev:cuda     # or: npm run tauri:build:cuda
```

It defaults `CMAKE_CUDA_ARCHITECTURES` to the detected GPU's compute capability
(override with `CMAKE_CUDA_ARCHITECTURES=89 npm run tauri:build:cuda`). At load
time the server auto-fits the GPU plan to the memory actually free.

To build a CUDA variant locally by hand, point the build at the CUDA toolkit and
— for a much faster compile — at just your GPU's compute capability (`86` is
Ampere / RTX 30-series; `75` Turing, `80` A100, `89` Ada, `90` Hopper):

```bash
export PATH="/usr/local/cuda-12.6/bin:$PATH" \
       CUDA_PATH=/usr/local/cuda-12.6 \
       CUDA_LIBRARY_PATH=/usr/local/cuda-12.6 \
       CMAKE_CUDA_ARCHITECTURES=86
npm run tauri:build --prefix demo -- --features cuda --bundles deb
```

The installer lands in `demo/src-tauri/target/release/bundle/deb/` (the demo is
its own Cargo workspace); it installs the llama.cpp shared libraries to
`/usr/lib/riga`. Drop `--bundles deb` to build every Linux format. On an Apple
Silicon Mac the same command with `--features metal` builds the Metal variant
(no extra flags beyond Xcode's command-line tools).

The same on-demand `linux-cuda` release job also publishes a
`RIGA-CLI-CUDA-x86_64.tar.gz` — the `riga-cli` binary plus the llama.cpp/ggml
shared libraries. Extract it and run `./riga-cli --tui`; it needs the same
NVIDIA driver and CUDA runtime libraries as the desktop build.

`CUDA_LIBRARY_PATH` matters when more than one CUDA toolkit is installed: it is
the only variable the build script uses for the linker's library search order,
so it pins `-lcuda`/`-lcudart` to that toolkit. Pass the CUDA **root**
(the script appends `lib64` and `lib64/stubs` itself). Without it the linker
falls back to `/usr/local/cuda`, and a newer toolkit there can fail the link
with `undefined symbol: cudaGetDeviceProperties_v2` (CUDA 13 dropped the `_v2`
suffix).

The build script only watches `CUDA_PATH`, not `CUDA_LIBRARY_PATH`, so after
changing it force the cached build script to re-run (a clean CI build does this
automatically). Dropping the cached output re-runs the script and re-emits the
link order without recompiling the CUDA kernels; `cargo clean -p` is the
guaranteed fallback but recompiles them:

```bash
rm -rf target/release/build/llama-cpp-sys-2-*/output
# fallback: cargo clean -p llama-cpp-sys-2
```

## Provider configuration

Configure an OpenAI-compatible endpoint, model, reasoning effort, and API key in the UI. Provider metadata is restored from the encrypted server store; the key is never returned to the browser in restoration frames.

Set `RIGA_TOKEN` to the encryption secret used by the server-side store. Use a deployment secret manager in production.

### OpenCode Responses API adapter

`packages/opencode` is a standalone TypeScript server that uses the official
[`@opencode-ai/sdk`](https://opencode.ai/docs/sdk/) (`@opencode-ai/sdk/v2`) to
start OpenCode and expose its configured providers through an OpenAI
Responses-compatible API. Install and configure the OpenCode CLI first, then
from the repository root run:

```bash
npm install
npm run dev --workspace @rigai/opencode
```

The adapter listens on `http://127.0.0.1:8788/v1` by default, while the SDK
starts the OpenCode server on `127.0.0.1:4096`. Build and run the compiled
package with `npm run build --workspace @rigai/opencode` followed by
`npm run start --workspace @rigai/opencode`. To connect to an OpenCode server
that is already running instead, set `OPENCODE_BASE_URL` and the adapter will
attach rather than spawning another server. OpenCode v2 requires HTTP Basic
authentication, so also set `OPENCODE_PASSWORD` (the server password) when
attaching; the adapter authenticates as user `opencode`.

The adapter implements `GET /v1/models`, `POST /v1/responses`, and
`GET /v1/responses/{id}`. Model IDs use OpenCode's `provider/model` convention.
The adapter returns the OpenCode session ID directly as both `response.id` and
`response.conversation.id`; a supplied `previous_response_id` is used directly as
the OpenCode session ID. Without one, the adapter creates a new OpenCode session.
The proxy therefore does not need a response-ID-to-session mapping. It retains
response snapshots only for `GET /v1/responses/{id}`.

RIGA persists the mapping from each RIGA session to the upstream session ID for
each provider registration. It is stored as `provider_sessions` using RIGA's
configured database, encrypted file store (`RIGA_TOKEN`), or local JSON fallback.
The registration identity is derived from the endpoint, backend/API type, and a
hash of the API key; the model is excluded, so changing models within the same
provider keeps the conversation, while changing providers or credentials starts
a separate provider-side conversation. When RIGA has a mapping, it sends that
ID as `previous_response_id` and sends only the current turn; on the first turn
for that provider registration it omits the ID, seeds the new session with any
RIGA transcript history, then saves the returned ID. Deleting a RIGA session
removes its local mappings but does not delete provider-side sessions.

The adapter accepts text input and returns text; OpenCode tool execution remains
inside its session. `stream: true` now streams as OpenCode works: assistant text
arrives as `response.output_text.delta`, reasoning as
`response.riga.reasoning.delta`, and tool activity as
`response.riga.tool_call.started` / `response.riga.tool_call.finished`, so a long
tool-using turn is visible instead of arriving as one buffered blob.

OpenCode's human-in-the-loop prompts are forwarded too. OpenCode's built-in
`build` agent allows every tool by default, so when the adapter launches its own
OpenCode server it ships a default `OPENCODE_CONFIG_CONTENT` that asks for
`shell` and `edit`; RIGA's approval card is therefore the gate for those
mutations instead of them running silently. When OpenCode asks, the stream emits
`response.riga.approval.requested`; the client answers with
`POST /v1/approvals/{id}` and a JSON body of `{"reply":"once"|"always"|"reject"}`,
which resumes the waiting turn. OpenCode's `form` tool (its interactive-question
mechanism) uses the same round trip: approving selects each field's first option
or boolean default, and rejecting cancels the form.

Set `OPENCODE_CONFIG_CONTENT` to override the default permission rules, and set
`OPENCODE_BASE_URL` to attach to an existing server (the adapter cannot inject
permissions into a server it did not start, so that server's own rules apply).

Set `RIGA_OPENCODE_HOST` / `RIGA_OPENCODE_PORT` to change the API bind address;
`OPENCODE_HOST` / `OPENCODE_PORT` configure the SDK-launched OpenCode server.
`RIGA_OPENCODE_API_KEY` enables bearer-token authentication and is required when
the adapter binds beyond localhost/loopback. State defaults to
`~/.local/share/riga/opencode/responses.json` on Linux and can be redirected with
`RIGA_OPENCODE_STATE_PATH` or `RIGA_OPENCODE_DATA_DIR`.

Example request:

```bash
curl http://127.0.0.1:8788/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"provider/model","input":"Summarize this project"}'
```

Replace `provider/model` with an exact ID from `GET /v1/models`. Use the returned
response `id` as `previous_response_id` on the next request. Configure multiple
upstream providers and credentials in OpenCode using its
[model-provider setup](https://opencode.ai/docs/models/); the adapter lists their
available models as `provider/model` IDs. When using RIGA as a client,
configure its remote endpoint as `http://127.0.0.1:8788/v1`, choose the
**Responses API** mode, and select one of those provider/model IDs. RIGA's model
picker fetches and groups that catalog from the configured endpoint instead of
using a hard-coded list.

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

Use `task` with `action: "agents"` to discover profiles or `action: "dispatch"` with an `agent` and `prompt` to request a structured handoff. A dispatch may include an optional `model` override, such as `provider/model` when using the OpenCode adapter; otherwise the configured subagent/main-model policy applies. The parent RIGA run remains responsible for tool execution and approvals, so the profiles do not bypass workspace policy gates.

Attachments are uploaded through `POST /attachments` and stored under `tmp/riga-attachments/`. The generated workspace-relative path is included in the agent prompt so the read tool can inspect the file. Runtime attachments are ignored by Git.

## Validation

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check
npm test
npm install --prefix demo
npm run build --prefix demo
cargo check --manifest-path demo/src-tauri/Cargo.toml --locked
```

## Publishing

Publish manually, in dependency order. Crates first — crates.io rejects a crate
whose dependencies are not yet published:

```bash
cargo publish -p riga-kernel
cargo publish -p riga-shell
cargo publish -p riga-server   # depends on riga-kernel
cargo publish -p riga-cli      # depends on riga-kernel and riga-server

npm publish --workspace @rigai/assistant-ui --access public
```

`cargo publish --dry-run -p <crate>` and
`npm publish --workspace @rigai/assistant-ui --dry-run` preview without
uploading. The assistant package’s `protocol`, `http`, and `tauri` exports are
part of the published package. `demo/README.md` documents switching the demo
between the local sources and these published releases.

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
