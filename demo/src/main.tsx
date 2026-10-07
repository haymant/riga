import ReactDOM from "react-dom/client";
import App from "./App";

// No `React.StrictMode`: it double-invokes effects in development, so the
// assistant transport is created twice and both sockets deliver the same deltas
// — every token renders twice. The transport is an external resource, so the
// app mounts once.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<App />);
