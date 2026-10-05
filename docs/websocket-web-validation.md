

## Real provider integration update

The previous deterministic WebSocket bridge has been replaced for normal runs. The browser can now send an ephemeral `configure_provider` frame containing:

```json
{
  "type": "configure_provider",
  "endpoint": "https://provider.example/v1",
  "api_key": "provided-at-runtime",
  "model": "opencode-go"
}
```

The server keeps this configuration only in the active WebSocket task. It is not written to disk, environment files, session journals, or logs. `start_run` then calls `POST {endpoint}/chat/completions` with the selected model and prompt and maps the response into RIGA `RunStarted`, `TextDelta`, and `RunCompleted` events. Provider failures become `RunFailed` events rather than silent UI failures.

The public UI now has **Settings → OpenAI-compatible / OpenCode Go**, with endpoint, API-key, and model fields. The browser test configured a local OpenAI-compatible mock and received `Mock provider response for: What provider are you using?` through the visible UI.

## Local model download scope

Fina Builder’s design was followed: GGUF download, checksum verification, model storage, CPU/OpenMP inference, and optional CUDA compilation are desktop/Tauri responsibilities. Browser + HTTP mode cannot safely access the host filesystem or bind directly to the user’s GPU. The RIGA settings UI therefore exposes a **Local GGUF model** mode with an explicit desktop-only explanation instead of pretending browser mode can download or run a local model.

The next desktop phase should add a Tauri model manager with a signed catalog, cancellable download to app data, byte-count and SHA-256 verification, atomic finalization, and a `llama-cpp-2` feature split for portable CPU versus opt-in CUDA builds. The browser provider path is already independent of that future local runtime.

Reference reviewed: [Fina Builder](https://github.com/haymant/fina-builder).
