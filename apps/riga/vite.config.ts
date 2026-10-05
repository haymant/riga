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
      "/sessions": { target: "http://127.0.0.1:8787" },
    },
  },
  build: { target: "es2022", outDir: "dist" },
});
