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

### Local vs published dependencies (parity)

The demo consumes RIGA over two channels, and both can point at either the
in-repo sources or the published releases:

| Dependency | Local (default) | Published |
| --- | --- | --- |
| `@rigai/assistant-ui` (npm) | `file:../packages/assistant-ui` | `^<version>` from npmjs |
| `riga-kernel`, `riga-server`, `riga-shell` (Rust) | `path = "../../crates/*"` | `<version>` from crates.io |

Switch modes with the helper — run it from `demo/`, or from the repository root
with `--prefix demo`:

```bash
npm run deps:status      # show the current mode
npm run deps:published   # use npmjs + crates.io
npm run deps:local       # use the in-repo sources (default)
```

`scripts/deps.mjs` rewrites `demo/package.json` and `demo/src-tauri/Cargo.toml`
in place, taking the published versions from the workspace
(`Cargo.toml` → `[workspace.package].version`) and
`packages/assistant-ui/package.json`, so the switch always targets the release
that matches the current source tree. A separate `package.json` alone is **not**
enough — the Rust path dependencies and both lockfiles have to move too.

Because the dependency *sources* change, reinstall afterwards and let Cargo
re-resolve:

```bash
npm install --prefix demo
cargo update --manifest-path demo/src-tauri/Cargo.toml
```

Both lockfiles (`demo/package-lock.json`, `demo/src-tauri/Cargo.lock`) are
tracked. Keep the default **local** mode committed; switching is for local
parity checks. To undo a switch without a reinstall, revert the manifests:

```bash
git checkout -- demo/package.json demo/src-tauri/Cargo.toml
```

Unlike the npm side, a standalone `package.json` cannot switch the Rust
dependency graph on its own — hence the helper covers both files. See
[§7](#7-publishing-the-riga-dependencies) for how to publish the releases that
the published mode resolves against.

`src-tauri/src/main.rs` calls `riga_shell::prepare_linux_display()` before the Tauri/GTK runtime starts. That helper (in `crates/riga-shell`) sets conservative cursor defaults and prefers XWayland when a `DISPLAY` is present, avoiding a Wayland cursor-theme assertion that otherwise aborts the app before the window appears. It is a native-shell concern, so it lives there rather than in `riga-kernel`, `riga-server`, or the web UI.

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

## 5. Android and iOS preview

The Tauri shell already has the mobile entry point and Vite's `TAURI_DEV_HOST` handling. Initialize the generated native project from `demo/` on the relevant host. Android requires Android Studio's SDK, platform tools, NDK, and Java 17; iOS requires macOS with Xcode and CocoaPods.

```bash
cd demo
npm ci
npm run tauri -- android init       # Android, run once
npm run tauri -- android dev        # deploy to an attached device or emulator
npm run tauri -- android build --debug --apk

# On macOS only:
npm run tauri -- ios init           # run once
npm run tauri -- ios dev            # deploy to an attached device or simulator
npm run tauri -- ios build
```

The manually triggered **Publish Android preview APK** workflow initializes the Android project on its runner and creates a GitHub prerelease containing a debug APK. It is intended for hands-on testing and uses the runner's debug signing key, not a persistent release keystore. Reinstalling a later preview may require uninstalling the earlier one. The workflow does not publish to Google Play.

Mobile is currently a **remote-provider chat preview**, not the full desktop coding-agent feature set. The mobile host stores RIGA data and the workspace inside the OS-managed app-data sandbox and skips desktop Git initialization. CUDA is desktop-only. Local GGUF inference, shell execution, Git operations, and MCP stdio servers depend on native libraries or executables that are not supplied in the mobile app; do not treat those features as supported on Android/iOS. The GitHub Actions build validates Android packaging, but device/emulator behavior still needs manual testing. iOS builds and testing require a Mac with Xcode.

### CUDA (NVIDIA) desktop

To run the desktop shell against the GPU (llama.cpp offloads, and the server
auto-fits the plan to the memory actually free), use the wrapper instead of the
plain Tauri command — it sets up the toolkit, the position-independent CUDA
objects the cdylib needs, and a glibc/CUDA header shim:

```bash
npm run tauri:dev:cuda     # or: npm run tauri:build:cuda
```

It defaults `CMAKE_CUDA_ARCHITECTURES` to the detected GPU; override it with
`CMAKE_CUDA_ARCHITECTURES=89 npm run tauri:build:cuda`.

## 6. Build and validate

```bash
npm run typecheck --prefix demo
npm run build --prefix demo
cargo check --manifest-path demo/src-tauri/Cargo.toml --locked
```

The verified build produces `demo/dist/` and compiles the Tauri host with the in-process IPC service.

## 7. Generated-app baseline

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

## 8. Publishing the RIGA dependencies

The published mode in [§2](#local-vs-published-dependencies-parity) resolves
against releases, so publish the packages first. Crates go out in dependency
order — crates.io rejects a crate whose dependencies are not yet published:

```bash
# Rust crates, from the repository root, dependency order
cargo publish -p riga-kernel
cargo publish -p riga-shell
cargo publish -p riga-server   # depends on riga-kernel
cargo publish -p riga-cli      # depends on riga-kernel and riga-server

# npm package; the @rigai scope needs --access public
npm publish --workspace @rigai/assistant-ui --access public
```

Preview before publishing:

```bash
cargo publish --dry-run -p riga-kernel
npm publish --workspace @rigai/assistant-ui --dry-run
```

If a version already exists, bump it first in `Cargo.toml`
(`[workspace.package].version`) and `packages/assistant-ui/package.json`, then
publish again. After publishing, verify parity end to end:

```bash
npm run deps:published --prefix demo
npm install --prefix demo
cargo build --manifest-path demo/src-tauri/Cargo.toml
```

Restore the default local mode when finished:

```bash
npm run deps:local --prefix demo && npm install --prefix demo
```
