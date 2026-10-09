import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["packages/assistant-ui/src/**/*.test.ts", "packages/assistant-ui/src/**/*.test.tsx"],
    coverage: {
      provider: "v8",
      include: ["packages/assistant-ui/src/http/**/*.ts", "packages/assistant-ui/src/tauri/**/*.ts", "packages/assistant-ui/src/graphAdapter.ts"],
      exclude: ["**/*.test.ts"],
      reporter: ["text", "json-summary"],
      thresholds: { lines: 80, functions: 80, branches: 75, statements: 80 },
    },
  },
});
