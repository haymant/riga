import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  resolve: {
    dedupe: ["react", "react-dom"],
    alias: {
      "@rigai/assistant-ui/tauri": new URL("../packages/assistant-ui/src/tauri/index.ts", import.meta.url).pathname,
      "@rigai/assistant-ui/protocol": new URL("../packages/assistant-ui/src/protocol/index.ts", import.meta.url).pathname,
      "@rigai/assistant-ui/http": new URL("../packages/assistant-ui/src/http/index.ts", import.meta.url).pathname,
      "@rigai/assistant-ui": new URL("../packages/assistant-ui/src/index.ts", import.meta.url).pathname,
      },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || "0.0.0.0",
    allowedHosts: true,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
    // The browser development client talks to the standalone `riga-server` over
    // HTTP/WebSocket on 8787. The Tauri desktop client does not use this proxy:
    // it talks to the in-process server over IPC (see `src/App.tsx`, which uses
    // `@rigai/assistant-ui/tauri`). The proxy exists only for `npm run dev`.
    proxy: {
      "/ws": { target: "ws://127.0.0.1:8787", ws: true },
      "/health": { target: "http://127.0.0.1:8787" },
      "/catalog": { target: "http://127.0.0.1:8787" },
      "/mcp": { target: "http://127.0.0.1:8787" },
      "/attachments": { target: "http://127.0.0.1:8787" },
      "/sessions": { target: "http://127.0.0.1:8787" },
      "/local-models": { target: "http://127.0.0.1:8787" },
    },
  },
}));
