// The three node types drawn on the canvas. Handles match what the server accepts:
// sources and filters connect onwards; a filter has "match" and "else" outputs;
// a target's only output is "fallback", to another target.

import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import type { FilterData, HttpData, SourceData } from "../api";
import { opLabel } from "./conditions";

type Flags = { hasToken?: boolean; stats?: { delivered: number; failed: number } };
export type SourceNodeT = Node<SourceData & Flags, "source">;
export type FilterNodeT = Node<FilterData, "filter">;
export type HttpNodeT = Node<HttpData & Flags, "http">;

export function SourceNode({ data, selected }: NodeProps<SourceNodeT>) {
  return (
    <div className={`node node-source ${selected ? "selected" : ""}`}>
      <div className="node-kind">Source</div>
      <div className="node-title">{data.name}</div>
      <div className={`node-meta ${data.hasToken ? "" : "warn"}`}>
        {data.hasToken ? "Token set" : "No token yet"}
      </div>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}

export function FilterNode({ data, selected }: NodeProps<FilterNodeT>) {
  const summary = data.conditions.length
    ? data.conditions
        .slice(0, 3)
        .map((c) => `${c.field} ${opLabel(c.op)} ${c.op.endsWith("exists") ? "" : JSON.stringify(c.value)}`)
    : ["everything"];
  return (
    <div className={`node node-filter ${selected ? "selected" : ""}`}>
      <Handle type="target" position={Position.Left} />
      <div className="node-kind">Filter · {data.mode === "any" ? "any of" : "all of"}</div>
      <div className="node-title">{data.name}</div>
      <ul className="node-conditions">
        {summary.map((s, i) => (
          <li key={i}>{s}</li>
        ))}
        {data.conditions.length > 3 && <li>+{data.conditions.length - 3} more</li>}
      </ul>
      <div className="handle-label handle-label-match">match</div>
      <div className="handle-label handle-label-else">else</div>
      <Handle type="source" id="match" position={Position.Right} className="handle-match" style={{ top: "35%" }} />
      <Handle type="source" id="else" position={Position.Right} className="handle-else" style={{ top: "75%" }} />
    </div>
  );
}

const presetIcon: Record<string, string> = { discord: "D", "claude-monitor": "M", webhook: "↗" };

export function HttpNode({ data, selected }: NodeProps<HttpNodeT>) {
  let host = data.url;
  try {
    host = new URL(data.url).host || data.url;
  } catch {
    /* a template or unfinished URL */
  }
  return (
    <div className={`node node-http ${selected ? "selected" : ""}`}>
      <Handle type="target" position={Position.Left} />
      <div className="node-kind">
        <span className="preset-icon">{presetIcon[data.preset ?? "webhook"] ?? "↗"}</span> Target · {data.method}
      </div>
      <div className="node-title">{data.name}</div>
      <div className="node-meta mono">{host}</div>
      <div className="node-meta">
        {data.retry.maxAttempts} attempts within {Math.round(data.retry.giveUpAfterSecs / 60)} min
      </div>
      {data.stats && (data.stats.delivered > 0 || data.stats.failed > 0) && (
        <div className="node-stats">
          <span className="ok">{data.stats.delivered} sent</span>
          {data.stats.failed > 0 && <span className="bad">{data.stats.failed} failed</span>}
        </div>
      )}
      <div className="handle-label handle-label-fallback">fallback</div>
      <Handle type="source" id="fallback" position={Position.Bottom} className="handle-fallback" />
    </div>
  );
}

export const nodeTypes = { source: SourceNode, filter: FilterNode, http: HttpNode };
