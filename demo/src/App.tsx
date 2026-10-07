import { AssistantUI } from "@rigai/assistant-ui";
import { createTauriTransport } from "@rigai/assistant-ui/tauri";
import "./App.css";

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function App() {
  const desktop = isTauriRuntime();
  return (
    <main className="riga-demo">
      <section className="assistant-host" aria-label="RIGA assistant UI">
        <AssistantUI
          showSessionHistoryButton
          fullWidth
          transportFactory={desktop ? (listeners) => createTauriTransport({ listeners }) : undefined}
        />
      </section>
    </main>
  );
}

export default App;
