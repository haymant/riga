import { describe, expect, it } from "vitest";
import { graphStateLabel, toExecutionFlow, type GraphNodeInput } from "./graphAdapter";

const graph: GraphNodeInput[] = [
  { id: "runtime", profile: "explore", description: "Inspect runtime", prompt: "inspect", depends_on: [], state: "done" },
  { id: "ui", profile: "explore", description: "Inspect UI", prompt: "inspect", depends_on: [], state: "running" },
  { id: "review", profile: "review", description: "Review findings", prompt: "review", depends_on: ["runtime", "ui"], state: "blocked", blockedBy: ["ui"] },
];

describe("execution graph adapter", () => {
  it("maps nodes into dependency-level rows and edges", () => {
    const result = toExecutionFlow(graph);
    expect(result.nodes.map((node) => [node.id, node.position])).toEqual([
      ["runtime", { x: 0, y: 0 }],
      ["ui", { x: 0, y: 100 }],
      ["review", { x: 260, y: 0 }],
    ]);
    expect(result.edges).toHaveLength(2);
    expect(result.edges.find((edge) => edge.id === "ui->review")?.className).toContain("blocked");
    expect(result.edges.find((edge) => edge.id === "runtime->review")?.className).toContain("satisfied");
  });

  it("filters to a focused node and its dependents", () => {
    const result = toExecutionFlow(graph, "runtime");
    expect(result.nodes.map((node) => node.id)).toEqual(["runtime", "review"]);
    expect(result.edges.map((edge) => edge.id)).toEqual(["runtime->review"]);
  });

  it("keeps state and blocked dependencies in node data", () => {
    const result = toExecutionFlow(graph);
    const review = result.nodes.find((node) => node.id === "review");
    expect(review?.data.state).toBe("blocked");
    expect(review?.data.blockedBy).toEqual(["ui"]);
    expect(review?.className).toBe("execution-node node-blocked");
  });

  it("uses user-facing labels for every task state", () => {
    expect(graphStateLabel("pending")).toBe("ready");
    expect(graphStateLabel("waiting")).toBe("waiting for approval");
    expect(graphStateLabel("done")).toBe("completed");
    expect(graphStateLabel("failed")).toBe("failed");
  });
});
