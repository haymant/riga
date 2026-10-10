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

To build and start the CLI with NVIDIA CUDA offload, enable the server's
dependency feature through the CLI package and provide a compatible CUDA
toolkit and NVIDIA driver on the build and runtime machines:

```bash
export PATH="/usr/local/cuda-12.6/bin:$PATH"
export CUDA_PATH=/usr/local/cuda-12.6
export CUDA_LIBRARY_PATH=/usr/local/cuda-12.6
export CMAKE_CUDA_ARCHITECTURES=86  # change to the GPU's compute capability
cargo run -p riga-cli --features riga-server/cuda -- --tui
```

Use `cargo clean -p llama-cpp-sys-2` after changing CUDA toolkit paths or the
CUDA feature. CUDA is a compile-time backend choice; merely running the CPU
binary on a CUDA-capable machine does not enable GPU offload. The server still
auto-fits local-model layers to currently available GPU memory. If no CUDA
toolkit is installed, omit the feature and use the CPU command above.

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

Never commit `.env.local`, provider keys, `RIGA_TOKEN`, or uploaded files. The API key is encrypted at rest by the server store and is write-only from the browser.

### Reference demo and desktop shell

```bash
npm install --prefix demo
npm run dev --prefix demo       # browser HTTP/WebSocket mode
npm run tauri dev --prefix demo # desktop Tauri IPC mode
```

`demo/` is the reference consumer. Browser mode starts `riga-server` on `127.0.0.1:8787` and uses the Vite proxy. Tauri mode starts `riga-server` in-process and passes `createTauriTransport` to `AssistantUI`; assistant operations use only `invoke` and Tauri events, with no HTTP loopback fallback. See [`demo/README.md`](demo/README.md) for the complete step-by-step integration and provider/model checks.

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
exception-specification clash:

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
its own Cargo workspace). Drop `--bundles deb` to build every Linux format, but
the rpm and AppImage steps are slow on the large CUDA binary. On an Apple
Silicon Mac the same command with `--features metal` builds the Metal variant
(no extra flags beyond Xcode's command-line tools).

`CUDA_LIBRARY_PATH` matters when more than one CUDA toolkit is installed: it is
the only variable the build script uses for the linker's library search order,
so it pins `-lcuda`/`-lcudart_static` to that toolkit. Pass the CUDA **root**
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
npm install --prefix demo
npm run build --prefix demo
cargo check --manifest-path demo/src-tauri/Cargo.toml --locked
```

## Publishing

Rust crates are published from the workspace in dependency order:

```bash
cargo publish --dry-run -p riga-kernel
cargo publish --dry-run -p riga-server
cargo publish --dry-run -p riga-cli
```

The release workflow publishes crates on `v*` tags when `CARGO_REGISTRY_TOKEN` is configured. Public npm packages use the `@rigai` scope and publish through the release workflow when `NPM_TOKEN` is configured. The assistant package’s `protocol`, `http`, and `tauri` exports are part of the published package.

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
