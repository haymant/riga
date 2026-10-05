import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["packages/transport-http/src/**/*.test.ts"],
    coverage: {
      provider: "v8",
      include: ["packages/transport-http/src/**/*.ts"],
      exclude: ["**/*.test.ts"],
      reporter: ["text", "json-summary"],
      thresholds: { lines: 80, functions: 80, branches: 75, statements: 80 },
    },
  },
});
