import { Background, Controls, MiniMap, ReactFlow, type NodeMouseHandler } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { graphStateLabel, toExecutionFlow, type GraphNodeInput } from "./graphAdapter";

export function RunGraphPanel({ nodes, focusNode, onFocusNode }: { nodes: GraphNodeInput[]; focusNode: string | null; onFocusNode: (nodeId: string) => void }) {
  const flow = toExecutionFlow(nodes, focusNode);
  const onNodeClick: NodeMouseHandler = (_event, node) => onFocusNode(node.id);
  return (
    <section className="run-graph-panel" aria-label="Interactive execution graph">
      <div className="agent-card-head"><strong>Execution graph</strong><span>{flow.nodes.length} node{flow.nodes.length === 1 ? "" : "s"}</span></div>
      <div className="run-graph-canvas" data-testid="run-graph-canvas">
        <ReactFlow nodes={flow.nodes} edges={flow.edges} onNodeClick={onNodeClick} fitView nodesConnectable={false} minZoom={0.5} maxZoom={1.5}>
          <Background gap={20} size={1} />
          <Controls showInteractive={false} />
          <MiniMap pannable zoomable nodeColor={(node) => node.data?.state === "done" ? "var(--green)" : node.data?.state === "blocked" ? "var(--amber)" : "var(--accent)"} />
        </ReactFlow>
      </div>
      <div className="run-graph-fallback" aria-label="Execution graph dependency list">
        {nodes.map((node) => <button type="button" className={`run-deck-node node-${node.state}`} key={node.id} onClick={() => onFocusNode(node.id)}><span><strong>{node.profile}</strong><small>{node.description}</small></span><span className="run-deck-node-state">{graphStateLabel(node.state)}{node.blockedBy?.length ? ` · blocked by ${node.blockedBy.join(", ")}` : ""}</span></button>)}
      </div>
    </section>
  );
}
