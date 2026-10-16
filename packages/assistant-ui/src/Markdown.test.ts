import { describe, expect, it } from "vitest";
import { splitThinking } from "./Markdown";

describe("splitThinking", () => {
  it("extracts a completed think block", () => {
    expect(splitThinking("before<think>reasoning</think>after")).toEqual([
      { thinking: false, content: "before" },
      { thinking: true, content: "reasoning" },
      { thinking: false, content: "after" },
    ]);
  });

  it("treats an unterminated block as still-streaming reasoning", () => {
    expect(splitThinking("before<think>partial")).toEqual([
      { thinking: false, content: "before" },
      { thinking: true, content: "partial" },
    ]);
  });

  it("accepts the thinking/reasoning/thought aliases", () => {
    expect(splitThinking("<reasoning>x</reasoning>ok")).toEqual([
      { thinking: true, content: "x" },
      { thinking: false, content: "ok" },
    ]);
  });

  it("turns a stray closing tag into a paragraph break", () => {
    expect(splitThinking("done </think> now")).toEqual([{ thinking: false, content: "done \n\n now" }]);
  });
});
