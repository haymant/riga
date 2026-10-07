# RIGA demo

`demo/` is the reference React + Tauri consumer for RIGA. It is generated with `create-tauri-app` and keeps the assistant UI thin: application behavior remains in `riga-server`, while the browser and desktop transports only frame requests and events.

## 1. Prerequisites

- Node.js 22+ and npm
- Rust 1.99.0+
- Tauri prerequisites for the target operating system
- An OpenAI-compatible endpoint, or a machine suitable for a local GGUF model

On Ubuntu, install GTK/WebKit development packages and `librsvg2-dev`.

## 2. Install the demo

From the repository root:

```bash
npm install --prefix demo
```

During development, the demo uses the current transport-injection source package:

```json
"@rigai/assistant-ui": "file:../packages/assistant-ui"
```

After the next assistant-ui release containing the injection API, replace it with the published version, for example:

```json
"@rigai/assistant-ui": "^0.1.4"
```

The Rust demo currently uses path dependencies for `riga-kernel` and `riga-server`. Replace those paths with the corresponding published crate versions when the IPC surface is released.

## 3. Browser development

Run the kernel and Vite together:

```bash
npm run dev --prefix demo
```

Open <http://localhost:1420>. Browser mode uses the HTTP/WebSocket adapter and Vite proxies `/health`, `/catalog`, `/sessions`, `/mcp`, `/attachments`, `/local-models`, and `/ws` to `riga-server` on `127.0.0.1:8787`.

To test an OpenAI-compatible provider:

1. Open **Settings**.
2. Choose the remote provider.
3. Enter the endpoint, API key, model, API mode, and reasoning effort.
4. Save the provider and send a message.

To test a local model:

1. Open **Settings → Local GGUF model**.
2. Refresh the catalog and download a model.
3. Wait for download progress to finish, then load the model.
4. Choose **Use this model** and send a message.

Provider settings and model state are persisted by the server’s configured backend.

## 4. Desktop development with Tauri IPC

Start the generated desktop shell with:

```bash
npm run tauri dev --prefix demo
```

The entrypoint detects the Tauri runtime and passes `createTauriTransport` to `AssistantUI`. Desktop assistant operations use only Tauri `invoke` commands and Tauri events:

- `riga://run-event`
- `riga://local-model-event`
- `riga://provider-configured`
- `riga://transport-error`

The desktop assistant path does **not** use `/ws`, `/health`, `/local-models`, loopback URLs, `fetch`, `EventSource`, or browser WebSocket APIs. The Vite HTTP proxy is retained only for browser development.

The Tauri host starts one `IpcService` around `ServerState`; it does not duplicate provider, tool, approval, MCP, attachment, session, or local-model logic. The same service powers the IPC commands and the HTTP/WebSocket routes.

## 5. Build and validate

```bash
npm run typecheck --prefix demo
npm run build --prefix demo
cargo check --manifest-path demo/src-tauri/Cargo.toml --locked
```

The verified build produces `demo/dist/` and compiles the Tauri host with the in-process IPC service.

## 6. Generated-app baseline

To recreate the initial shell used for this demo:

```bash
rm -rf demo
npm create tauri-app@latest demo -- \
  --template react-ts \
  --manager npm \
  --identifier dev.riga.demo \
  --yes
```

The generated shell was created by `create-tauri-app@4.7.4`; RIGA-specific integration is then added through the package and Rust dependencies described above.
