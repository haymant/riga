# RIGA desktop demo

This directory is the first independent consumer application for RIGA. It is intentionally created with the official Tauri application generator before adding RIGA-specific code.

## 1. Prerequisites

Install:

- Node.js 22+
- npm
- Rust 1.99.0+
- Tauri desktop prerequisites for your operating system

On Ubuntu, the Tauri prerequisites include GTK/WebKit development packages and `librsvg2-dev`.

## 2. Recreate the generated Tauri app

From the repository root:

```bash
rm -rf demo
npm create tauri-app@latest demo -- \
  --template react-ts \
  --manager npm \
  --identifier dev.riga.demo \
  --yes
```

The generator used for this app is `create-tauri-app@4.7.4`. It creates the React/Vite frontend, Rust Tauri host, icons, capability file, and build scripts.

## 3. Install dependencies

```bash
npm install --prefix demo
```

The generated application remains a private package named `riga`. The first integration uses a related filesystem path rather than a published npm package:

```json
{
  "dependencies": {
    "@rigai/assistant-ui": "file:../packages/assistant-ui"
  }
}
```

This is deliberate. It lets the demo validate the current source package before the release workflow switches it to a registry version.

## 4. Import the assistant UI

The React entrypoint imports the reusable surface directly from the related package:

```tsx
import { AssistantUI } from "@rigai/assistant-ui";

export function App() {
  return <AssistantUI showSessionHistoryButton fullWidth />;
}
```

`AssistantUI` owns its transcript, settings, session history controls, composer, model manager, and stylesheet. The host only supplies the surrounding layout and optional runtime properties.

## 5. Import the kernel from the related Rust path

The generated Tauri crate references the repository kernel without publishing a new crate first:

```toml
[dependencies]
riga-kernel = { path = "../../crates/riga-kernel" }
```

The demo registers a small `kernel_health` command backed by `riga_kernel::Agent`. The header displays the returned health value, proving that the generated Tauri shell and the RIGA kernel are connected independently of the assistant transport.

## 6. Validate the first integration

From the repository root:

```bash
npm install --prefix demo
npm run typecheck --prefix demo
npm run build --prefix demo
cargo check --manifest-path demo/src-tauri/Cargo.toml
```

For an interactive desktop check:

```bash
npm run tauri dev --prefix demo
```

Expected result:

1. A window titled **RIGA** opens.
2. The header shows a `kernel:` health result.
3. The RIGA assistant surface renders below the header.
4. Assistant requests can be tested through the Vite proxy using an OpenAI-compatible provider or a loaded local GGUF model.

## 7. Run the web app with the kernel

The browser development command starts both processes:

```bash
npm run dev
```

This runs:

- `riga-server` on `http://127.0.0.1:8787`
- Vite on `http://localhost:1420`

Vite proxies `/ws`, `/catalog`, `/sessions`, `/mcp`, `/attachments`, `/health`, and `/local-models` to the kernel. Open <http://localhost:1420> for manual verification.

### OpenAI-compatible endpoint

1. Open **Settings** in the RIGA surface.
2. Select the remote/OpenAI-compatible provider.
3. Enter the endpoint, API key, model, API mode, and reasoning effort.
4. Save the provider, then send a message.

Provider settings are sent through the WebSocket and persisted by the RIGA server according to its configured persistence backend.

### Local GGUF model

1. Open **Settings** and select **Local GGUF model**.
2. Refresh the curated catalog.
3. Download a model, wait for the download event to finish, and load it.
4. Choose **Use this model**, then send a message.

The browser demo uses the same `/local-models` HTTP and event-stream routes as the desktop shell, so download and load behavior can be validated before packaging.

## 8. Next integration stages

The changes are intentionally staged:

1. **Current:** generated Tauri app, related-path `riga-kernel`, related-path `@rigai/assistant-ui`.
2. Add the thin embedded `riga-server` adapter and provide its runtime URL to `AssistantUI`.
3. Validate Linux, Windows, and macOS builds from `demo/`.
4. Replace filesystem dependencies with published npm and crates.io versions.
5. Update the GitHub release workflow to build `demo/` and publish the RIGA desktop artifacts.

The demo must pass its source-path validation before step 4 changes any dependency to a registry version.
