# Embedding RIGA in a React + Tauri app

`apps/riga` is the reference embedding of RIGA. Almost nothing lives here: the
chat surface is a package, the agent is a kernel crate, and this app is the glue
that mounts them. Copy the three files below into any React + Tauri project to
get a working RIGA agent.

## What you are embedding

| Piece | Where | Role |
|---|---|---|
| `@haymant/assistant-ui` | `packages/assistant-ui` | The React chat surface: transcript, tool timeline, composer, settings and local-model manager, plus the stylesheet it is built from. |
| `riga-kernel::Agent` | `crates/riga-kernel` | The transport-neutral agent. A host owns one and forwards its own commands to it. |
| `riga-server` | `crates/riga-server` | HTTP + WebSocket host that runs the agent loop over the wire. The React surface talks to this, not to Tauri IPC. |

The two halves are independent: `Agent` is what you embed in Rust, and the React
surface is what you embed in the webview. The transport between them is
WebSocket, so the same surface works in a browser and in a Tauri window.

## What this app contains

```text
apps/riga
├── src/main.tsx              # mounts the React surface            (4 lines)
├── src-tauri/src/main.rs     # the whole embedding                 (~35 lines)
├── src-tauri/Cargo.toml
├── vite.config.ts            # proxies the surface to riga-server
└── src-tauri/tauri.conf.json # window, dev URL, dev CSP
```

## 1. Rust — embed the kernel agent

The kernel agent is `riga_kernel::Agent`. It is `Copy + Send + Sync`, so it drops
straight into Tauri's managed state, and it owns the behaviour so your shell
stays glue.

`src-tauri/Cargo.toml`:

```toml
[dependencies]
# In this repo use the path; from crates.io use `riga-kernel = "0.1"`.
riga-kernel = { path = "../../../crates/riga-kernel" }
tauri = { version = "2.0", default-features = false, features = ["wry"] }

[build-dependencies]
tauri-build = { version = "2.0", features = [] }
```

`build.rs` (unchanged from the Tauri template):

```rust
fn main() {
    tauri_build::build();
}
```

`src-tauri/src/main.rs` — the whole embedding:

```rust
// Hide the console window on Windows release builds; debug builds keep it so
// `tauri dev` output stays visible. Only means anything on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use riga_kernel::{Agent, Health};
use tauri::State;

/// The one command this shell exposes: prove the embedded agent is alive.
#[tauri::command]
fn health(agent: State<'_, Agent>) -> Health {
    agent.health()
}

fn main() {
    tauri::Builder::<tauri::Wry>::default()
        .manage(Agent::new())
        .invoke_handler(tauri::generate_handler![health])
        .run(tauri::generate_context!())
        .expect("error while running RIGA desktop application");
}
```

`health` is glue only — it holds no state and no logic. The payload type,
`Health`, is declared once in the kernel so a Tauri command and an HTTP route
cannot drift into returning different shapes for the same probe. The frontend
would call it with `invoke("health")` from `@tauri-apps/api/core`; this app does
not, because it drives the agent over WebSocket instead.

A single file is enough for desktop. Tauri's own template splits `run()` into
`lib.rs` because mobile targets link the library; add that back, plus
`[lib] crate-type = ["lib", "cdylib", "staticlib"]` in `Cargo.toml`, only if you
need iOS or Android.

## 2. React — mount the surface

`package.json`:

```json
{
  "dependencies": {
    "@haymant/assistant-ui": "file:../../packages/assistant-ui",
    "react": "^19.3.0",
    "react-dom": "^19.3.0"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.12.1",
    "@types/react": "^19.3.0",
    "@types/react-dom": "^19.3.0",
    "@vitejs/plugin-react": "^6.1.1",
    "vite": "^8.3.2"
  }
}
```

`src/main.tsx` — the whole entry point:

```tsx
import { createRoot } from "react-dom/client";
import { AssistantUI } from "@haymant/assistant-ui";

createRoot(document.getElementById("root")!).render(<AssistantUI />);
```

The component imports its own stylesheet, so there is nothing else to load. If
your host needs to control when the sheet loads, import it explicitly instead:

```tsx
import "@haymant/assistant-ui/styles.css";
```

`AssistantUI` accepts two options, both optional:

```tsx
<AssistantUI showSessionHistoryButton fullWidth />
```

## 3. Reach the agent — run `riga-server` and proxy to it

The surface resolves every request against the page origin, so in development
you run `riga-server` beside Vite and proxy the paths it uses:

| Path | Used for |
|---|---|
| `/ws` | the run control channel (WebSocket) |
| `/catalog` | tools, skills, agents, files, connectors |
| `/attachments` | uploaded files |
| `/mcp` | MCP connector registry |
| `/sessions` | durable session list |
| `/local-models` | GGUF catalog, downloads and load/unload |

Add them to `vite.config.ts`:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    proxy: {
      "/ws": { target: "ws://127.0.0.1:8787", ws: true },
      "/catalog": { target: "http://127.0.0.1:8787" },
      "/health": { target: "http://127.0.0.1:8787" },
      "/mcp": { target: "http://127.0.0.1:8787" },
      "/attachments": { target: "http://127.0.0.1:8787" },
      "/sessions": { target: "http://127.0.0.1:8787" },
      "/local-models": { target: "http://127.0.0.1:8787" },
    },
  },
  build: { target: "es2022", outDir: "dist" },
});
```

Each path is listed explicitly on purpose: a path missing from this list is
answered by Vite's SPA fallback with `index.html`, and the UI silently receives
HTML where it expected JSON.

Start the server with `cargo run -p riga-server` (it listens on
`RIGA_SERVER_ADDRESS`, default `127.0.0.1:8787`).

## 4. Run it

```bash
npm install
cp .env.example .env.local   # then set the flags below
npm run dev                  # riga-server + Vite on :1420
```

For a native window, `npm run tauri:dev` starts the same two processes through
`beforeDevCommand`. Do not run both at once: port `1420` is `strictPort` and both
claim it.

`tauri.conf.json` only needs the standard window and dev URL:

```json
{
  "build": {
    "frontendDist": "../dist",
    "devUrl": "http://127.0.0.1:1420",
    "beforeDevCommand": "npm run dev --prefix ../.."
  },
  "app": { "windows": [{ "label": "main", "title": "RIGA" }] }
}
```

### Approvals

Writes and shell commands are gated by the kernel's tool policy, not by
environment variables. The first time a run wants to write a file or run a
shell command it pauses, and the surface shows an approval card with **Allow
once**, **Always allow**, and **Decline**. "Always allow" is scoped to the
session; read-only tools never prompt, and destructive tools are denied.

Never commit `.env.local`, the provider key, or `RIGA_TOKEN`; provider
credentials are encrypted at rest by the server store and are write-only from
the browser.

## Extending

- **A new agent capability** belongs in the kernel. Add a method to `Agent` in
  `crates/riga-kernel/src/agent.rs` with a unit test, then forward to it from one
  `#[tauri::command]` (or one `riga-server` route). Keep the command free of
  logic so the behaviour stays testable in the kernel and reusable across
  transports.
- **A new UI affordance** belongs in `packages/assistant-ui`. The container app
  keeps only mounting and host configuration.

## Current limits

- The packaged (non-dev) Tauri build does not yet host the agent in-process. The
  shipped UI resolves `/ws` and the other paths against the `tauri://` origin,
  which needs the `packages/transport-tauri` boundary (currently a stub). In
  development the Vite proxy covers this, which is why `npm run dev` and
  `npm run tauri:dev` both run `riga-server` as a separate process.
- `riga-server` links `llama-cpp-2`, so the first kernel build compiles llama.cpp
  (about two minutes). `npm run dev` therefore starts slower than Vite alone.

## Further reading

- Root [`README.md`](../../README.md) — architecture, workspace layout, release flow.
- [`packages/assistant-ui`](../../packages/assistant-ui) — the React surface.
- [`crates/riga-kernel`](../../crates/riga-kernel) — the transport-free agent.
- [`crates/riga-server`](../../crates/riga-server) — the HTTP/WebSocket host.
