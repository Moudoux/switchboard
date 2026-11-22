import type { Op } from "../api";

export const fields = ["title", "body", "priority", "tags", "source", "url"] as const;

export const ops: { op: Op; label: string; needsValue: boolean }[] = [
  { op: "eq", label: "=", needsValue: true },
  { op: "ne", label: "≠", needsValue: true },
  { op: "contains", label: "contains", needsValue: true },
  { op: "not_contains", label: "doesn't contain", needsValue: true },
  { op: "matches", label: "matches regex", needsValue: true },
  { op: "gte", label: "≥", needsValue: true },
  { op: "gt", label: ">", needsValue: true },
  { op: "lte", label: "≤", needsValue: true },
  { op: "lt", label: "<", needsValue: true },
  { op: "exists", label: "is set", needsValue: false },
  { op: "not_exists", label: "is not set", needsValue: false },
];

export const opLabel = (op: Op) => ops.find((o) => o.op === op)?.label ?? op;

/** What the value box holds, turned into the JSON value the server compares with. */
export function parseValue(text: string, op: Op): unknown {
  if (["gt", "gte", "lt", "lte"].includes(op)) {
    const n = Number(text);
    return Number.isFinite(n) && text.trim() !== "" ? n : text;
  }
  return text;
}

export function valueText(value: unknown): string {
  if (value === null || value === undefined) return "";
  return typeof value === "string" ? value : JSON.stringify(value);
}
