import { describe, expect, it } from "vitest";
import { DEFAULT_OPENCODE_CONFIG, opencodeConfigContent } from "./config.js";

describe("adapter default OpenCode config", () => {
  it("asks for shell and edit so RIGA's approval gate applies", () => {
    expect(JSON.parse(DEFAULT_OPENCODE_CONFIG)).toEqual({
      permissions: [
        { action: "shell", resource: "*", effect: "ask" },
        { action: "edit", resource: "*", effect: "ask" },
      ],
    });
  });

  it("respects a caller-provided config and only falls back when unset", () => {
    const custom = '{"permission":{"shell":"allow"}}';
    expect(opencodeConfigContent(custom)).toBe(custom);
    expect(opencodeConfigContent(undefined)).toBe(DEFAULT_OPENCODE_CONFIG);
  });
});
