import { AssistantUI } from "@rigai/assistant-ui";
import "./App.css";

function App() {
  return (
    <main className="riga-demo">
      <section className="assistant-host" aria-label="RIGA assistant UI">
        <AssistantUI showSessionHistoryButton fullWidth />
      </section>
    </main>
  );
}

export default App;
