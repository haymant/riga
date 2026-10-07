import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AssistantUI } from "@rigai/assistant-ui";
import "./App.css";

function App() {
  const [kernelStatus, setKernelStatus] = useState("checking…");

  useEffect(() => {
    const healthRequest = "__TAURI_INTERNALS__" in window
      ? invoke("kernel_health")
      : fetch("/health").then((response) => {
          if (!response.ok) throw new Error(`health request failed (${response.status})`);
          return response.json();
        });
    void healthRequest
      .then((health) => setKernelStatus(JSON.stringify(health)))
      .catch((error: unknown) => setKernelStatus(`error: ${String(error)}`));
  }, []);

  return (
    <main className="riga-demo">
      <header className="riga-demo-header">
        <div>
          <p className="eyebrow">RIGA / TAURI DEMO</p>
          <h1>RIGA assistant surface</h1>
        </div>
        <code className="kernel-status" title="Result from the related-path riga-kernel crate">
          kernel: {kernelStatus}
        </code>
      </header>
      <section className="assistant-host" aria-label="RIGA assistant UI">
        <AssistantUI showSessionHistoryButton fullWidth />
      </section>
    </main>
  );
}

export default App;
