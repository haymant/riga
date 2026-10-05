import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    allowedHosts: true,
    proxy: {
      "/ws": { target: "ws://127.0.0.1:8787", ws: true },
      "/health": { target: "http://127.0.0.1:8787" },
      "/catalog": { target: "http://127.0.0.1:8787" },
      "/mcp": { target: "http://127.0.0.1:8787" },
      "/attachments": { target: "http://127.0.0.1:8787" },
      "/sessions": { target: "http://127.0.0.1:8787" },
      // Covers /local-models and all of its sub-paths, including the download
      // event stream. Without this the SPA fallback answers with index.html and
      // the model manager silently sees HTML instead of JSON.
      "/local-models": { target: "http://127.0.0.1:8787" },
    },
  },
  build: { target: "es2022", outDir: "dist" },
});
