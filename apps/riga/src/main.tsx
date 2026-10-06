import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { AssistantUI } from "@rigai/assistant-ui";

/**
 * Resolve where the RIGA server lives, then mount the surface against it.
 *
 * The packaged desktop app hosts `riga-server` inside the shell on an
 * OS-assigned loopback port; the `server_url` command reports that origin. The
 * browser build and `tauri:dev` get an empty string, which means "same origin"
 * and keeps the Vite proxy path working. Without this origin the packaged
 * webview resolves `/ws` and `/local-models` against `tauri://localhost` and
 * neither the socket nor the model list ever reaches the kernel.
 */
function App() {
  const [serverUrl, setServerUrl] = useState<string | undefined>(undefined);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    let cancelled = false;
    async function boot() {
      if ("__TAURI_INTERNALS__" in window) {
        try {
          const url = await invoke<string>("server_url");
          if (!cancelled) setServerUrl(url || undefined);
        } catch {
          // Fall back to same-origin; rendering the surface beats hanging on a
          // missing command.
        }
      }
      if (!cancelled) setReady(true);
    }
    void boot();
    return () => {
      cancelled = true;
    };
  }, []);

  if (!ready) {
    return (
      <main style={{ padding: 24, fontFamily: "system-ui, sans-serif" }}>
        Starting RIGA runtime…
      </main>
    );
  }

  return <AssistantUI serverUrl={serverUrl} />;
}

createRoot(document.getElementById("root")!).render(<App />);