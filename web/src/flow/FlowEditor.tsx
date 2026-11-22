// The routing graph editor: sources → filters → targets, with fallbacks between targets.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  addEdge,
  Background,
  BackgroundVariant,
  Controls,
  MarkerType,
  MiniMap,
  ReactFlow,
  ReactFlowProvider,
  useEdgesState,
  useNodesState,
  useReactFlow,
  type Connection,
  type Edge,
  type IsValidConnection,
  type Node,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { api, ApiError, describe, type Graph, type GraphNode, type GraphResponse, type NodeType } from "../api";
import { Inspector } from "./Inspector";
import { nodeTypes } from "./nodes";
import { newFilter, newSource, presets } from "./presets";

type AnyNode = Node<Record<string, unknown>, NodeType>;

const edgeStyle: Record<string, Partial<Edge>> = {
  match: { style: { stroke: "var(--match)", strokeWidth: 2 } },
  else: { style: { stroke: "var(--else)", strokeWidth: 2, strokeDasharray: "4 4" } },
  fallback: { style: { stroke: "var(--fallback)", strokeWidth: 2, strokeDasharray: "6 4" }, animated: true },
  default: { style: { stroke: "var(--edge)", strokeWidth: 2 } },
};

function decorate(edge: Edge): Edge {
  const kind = edge.sourceHandle ?? "default";
  return {
    ...edge,
    ...(edgeStyle[kind] ?? edgeStyle.default),
    markerEnd: { type: MarkerType.ArrowClosed, color: `var(--${kind === "default" ? "edge" : kind})` },
    label: kind === "fallback" ? "fallback" : undefined,
    labelStyle: { fill: "var(--fallback)", fontSize: 11 },
    labelBgStyle: { fill: "var(--bg)" },
  };
}

const newId = (type: string) => `${type}-${crypto.randomUUID().slice(0, 8)}`;

export function FlowEditor() {
  return (
    <ReactFlowProvider>
      <Editor />
    </ReactFlowProvider>
  );
}

function Editor() {
  const [nodes, setNodes, onNodesChange] = useNodesState<AnyNode>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const [loaded, setLoaded] = useState<GraphResponse>();
  const [dirty, setDirty] = useState(false);
  const [selected, setSelected] = useState<string>();
  const [savedIds, setSavedIds] = useState<Set<string>>(new Set());
  const [status, setStatus] = useState<{ kind: "error" | "ok"; text: string; details?: string[] }>();
  const [saving, setSaving] = useState(false);
  const [addMenu, setAddMenu] = useState(false);
  const flow = useReactFlow();
  const wrapper = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    const r = await api<GraphResponse>("/api/graph");
    setLoaded(r);
    setNodes(r.graph.nodes.map((n) => ({ ...n, data: { ...n.data } }) as AnyNode));
    setEdges(r.graph.edges.map((e) => decorate({ ...e, sourceHandle: e.sourceHandle ?? null })));
    setSavedIds(new Set(r.graph.nodes.map((n) => n.id)));
    setDirty(false);
  }, [setNodes, setEdges]);

  const refreshTokens = useCallback(async () => {
    const r = await api<GraphResponse>("/api/graph");
    setLoaded((l) => (l ? { ...l, tokens: r.tokens } : r));
  }, []);

  useEffect(() => {
    load().catch((e) => setStatus({ kind: "error", text: describe(e) }));
  }, [load]);

  useEffect(() => {
    if (!dirty) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  // Show on each source whether it has a token, without storing that in the graph.
  const shownNodes = useMemo(
    () =>
      nodes.map((n) =>
        n.type === "source" ? { ...n, data: { ...n.data, hasToken: Boolean(loaded?.tokens[n.id]) } } : n,
      ),
    [nodes, loaded],
  );

  const isValidConnection: IsValidConnection = useCallback(
    (c) => {
      const from = nodes.find((n) => n.id === c.source);
      const to = nodes.find((n) => n.id === c.target);
      if (!from || !to || from.id === to.id || to.type === "source") return false;
      if (from.type === "http") return to.type === "http" && c.sourceHandle === "fallback";
      return true;
    },
    [nodes],
  );

  const onConnect = useCallback(
    (c: Connection) => {
      setEdges((es) => {
        const exists = es.some(
          (e) => e.source === c.source && e.target === c.target && (e.sourceHandle ?? null) === (c.sourceHandle ?? null),
        );
        return exists ? es : addEdge(decorate({ ...c, id: newId("e") } as Edge), es);
      });
      setDirty(true);
    },
    [setEdges],
  );

  function place(): { x: number; y: number } {
    const box = wrapper.current?.getBoundingClientRect();
    const center = box ? { x: box.left + box.width / 2, y: box.top + box.height / 2 } : { x: 400, y: 300 };
    const p = flow.screenToFlowPosition(center);
    return { x: p.x - 90 + Math.random() * 40, y: p.y - 40 + Math.random() * 40 };
  }

  function add(type: NodeType, data: Record<string, unknown>) {
    const id = newId(type);
    setNodes((ns) => [...ns.map((n) => ({ ...n, selected: false })), { id, type, position: place(), data, selected: true }]);
    setSelected(id);
    setDirty(true);
    setAddMenu(false);
  }

  async function save() {
    if (!loaded) return;
    setSaving(true);
    setStatus(undefined);
    const graph: Graph = {
      nodes: nodes.map(({ id, type, position, data }) => {
        const { hasToken: _h, stats: _s, ...rest } = data as Record<string, unknown>;
        return { id, type: type as NodeType, position, data: rest } as GraphNode;
      }),
      edges: edges.map(({ id, source, target, sourceHandle }) => ({ id, source, target, sourceHandle: sourceHandle ?? null })),
    };
    try {
      const r = await api<{ version: number }>("/api/graph", { method: "PUT", json: { version: loaded.version, graph } });
      setLoaded({ ...loaded, version: r.version, graph });
      setSavedIds(new Set(graph.nodes.map((n) => n.id)));
      setDirty(false);
      setStatus({ kind: "ok", text: `Saved as version ${r.version}. New notifications use it now.` });
      refreshTokens();
    } catch (e) {
      if (e instanceof ApiError) setStatus({ kind: "error", text: e.message, details: e.details });
      else setStatus({ kind: "error", text: describe(e) });
    } finally {
      setSaving(false);
    }
  }

  const selectedNode = nodes.find((n) => n.id === selected);

  return (
    <div className="flow-page">
      <div className="toolbar">
        <button onClick={() => add("source", newSource())}>+ Source</button>
        <button onClick={() => add("filter", newFilter())}>+ Filter</button>
        <div className="menu-wrap">
          <button onClick={() => setAddMenu((v) => !v)}>+ Target ▾</button>
          {addMenu && (
            <div className="menu" onMouseLeave={() => setAddMenu(false)}>
              {presets.map((p) => (
                <button key={p.id} className="menu-item" onClick={() => add("http", p.make())}>
                  <strong>{p.label}</strong>
                  <span className="muted small">{p.description}</span>
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="spacer" />
        {loaded && <span className="muted small">version {loaded.version}</span>}
        {dirty && <span className="unsaved">Unsaved changes</span>}
        <button disabled={!dirty || saving} onClick={() => load()}>
          Discard
        </button>
        <button className="primary" disabled={!dirty || saving} onClick={save}>
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
      {status && (
        <div className={`banner ${status.kind}`}>
          {status.text}
          {status.details && (
            <ul>
              {status.details.map((d, i) => (
                <li key={i}>{d}</li>
              ))}
            </ul>
          )}
          <button className="icon" onClick={() => setStatus(undefined)}>
            ×
          </button>
        </div>
      )}
      <div className="flow-body">
        <div className="canvas" ref={wrapper}>
          <ReactFlow
            nodes={shownNodes}
            edges={edges}
            nodeTypes={nodeTypes}
            onNodesChange={(changes) => {
              onNodesChange(changes);
              if (changes.some((c) => c.type !== "select" && c.type !== "dimensions")) setDirty(true);
              for (const c of changes) if (c.type === "remove" && c.id === selected) setSelected(undefined);
            }}
            onEdgesChange={(changes) => {
              onEdgesChange(changes);
              if (changes.some((c) => c.type === "remove")) setDirty(true);
            }}
            onConnect={onConnect}
            isValidConnection={isValidConnection}
            onNodeClick={(_, n) => setSelected(n.id)}
            onPaneClick={() => setSelected(undefined)}
            deleteKeyCode={["Backspace", "Delete"]}
            fitView
            fitViewOptions={{ maxZoom: 1.1 }}
            proOptions={{ hideAttribution: true }}
          >
            <Background variant={BackgroundVariant.Dots} gap={20} size={1} color="var(--grid)" />
            <Controls showInteractive={false} />
            <MiniMap pannable zoomable nodeColor={(n) => `var(--node-${n.type})`} maskColor="var(--minimap-mask)" />
          </ReactFlow>
          {loaded && nodes.length === 0 && (
            <div className="empty-hint">
              <h3>Nothing routed yet</h3>
              <p>
                Add a <strong>source</strong> for each project that sends notifications, then <strong>targets</strong>{" "}
                to deliver to, and connect them. Put <strong>filters</strong> in between to route by priority, tags or
                anything in the notification. Drag from a target's bottom handle to another target to add a fallback.
              </p>
            </div>
          )}
        </div>
        {selectedNode && (
          <Inspector
            key={selectedNode.id}
            id={selectedNode.id}
            type={selectedNode.type as NodeType}
            data={selectedNode.data as never}
            saved={savedIds.has(selectedNode.id) && !dirty}
            token={loaded?.tokens[selectedNode.id]}
            onChange={(data) => {
              setNodes((ns) => ns.map((n) => (n.id === selectedNode.id ? { ...n, data: data as Record<string, unknown> } : n)));
              setDirty(true);
            }}
            onDelete={() => {
              flow.deleteElements({ nodes: [{ id: selectedNode.id }] });
              setSelected(undefined);
              setDirty(true);
            }}
            onTokenCreated={refreshTokens}
          />
        )}
      </div>
    </div>
  );
}
