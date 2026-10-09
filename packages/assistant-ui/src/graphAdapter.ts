import type { Edge, Node } from "@xyflow/react";

export type GraphNodeState = "pending" | "running" | "waiting" | "blocked" | "done" | "failed";

export type GraphNodeInput = {
  id: string;
  profile: string;
  description: string;
  prompt: string;
  depends_on: string[];
  state: GraphNodeState;
  progress?: number;
  blockedBy?: string[];
};

export type ExecutionNodeData = {
  label: string;
  profile: string;
  state: GraphNodeState;
  blockedBy: string[];
};

function nodeDepth(id: string, byId: Map<string, GraphNodeInput>, visiting = new Set<string>()): number {
  if (visiting.has(id)) return 0;
  const node = byId.get(id);
  if (!node || node.depends_on.length === 0) return 0;
  const next = new Set(visiting).add(id);
  return 1 + Math.max(...node.depends_on.map((dependency) => nodeDepth(dependency, byId, next)));
}

export function toExecutionFlow(nodes: GraphNodeInput[], focusNode: string | null = null): { nodes: Node<ExecutionNodeData>[]; edges: Edge[] } {
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const visible = focusNode
    ? nodes.filter((node) => node.id === focusNode || node.depends_on.includes(focusNode))
    : nodes;
  const rows = new Map<number, number>();
  const flowNodes = visible.map((node) => {
    const depth = nodeDepth(node.id, byId);
    const row = rows.get(depth) ?? 0;
    rows.set(depth, row + 1);
    return {
      id: node.id,
      type: "default",
      position: { x: depth * 260, y: row * 100 },
      data: {
        label: `${node.profile}\n${node.description}`,
        profile: node.profile,
        state: node.state,
        blockedBy: node.blockedBy ?? [],
      },
      className: `execution-node node-${node.state}`,
    };
  });
  const visibleIds = new Set(visible.map((node) => node.id));
  const edges = visible.flatMap((node) => node.depends_on
    .filter((dependency) => visibleIds.has(dependency))
    .map((dependency) => ({
      id: `${dependency}->${node.id}`,
      source: dependency,
      target: node.id,
      animated: node.state === "running",
      className: node.blockedBy?.includes(dependency) ? "execution-edge blocked" : "execution-edge satisfied",
    })));
  return { nodes: flowNodes, edges };
}

export function graphStateLabel(state: GraphNodeState): string {
  switch (state) {
    case "done": return "completed";
    case "failed": return "failed";
    case "blocked": return "blocked";
    case "running": return "running";
    case "waiting": return "waiting for approval";
    default: return "ready";
  }
}
