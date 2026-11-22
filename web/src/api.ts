// Typed calls to the switchboard server. Everything goes through `api()`.

export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
    public details: string[] = [],
  ) {
    super(message);
  }
}

export async function api<T>(path: string, init: RequestInit & { json?: unknown } = {}): Promise<T> {
  const { json, ...rest } = init;
  const res = await fetch(path, {
    ...rest,
    credentials: "same-origin",
    headers: json !== undefined ? { "Content-Type": "application/json", ...rest.headers } : rest.headers,
    body: json !== undefined ? JSON.stringify(json) : rest.body,
  });
  const text = await res.text();
  const body = text ? JSON.parse(text) : null;
  if (!res.ok) {
    if (res.status === 401 && !path.startsWith("/api/auth/")) window.dispatchEvent(new Event("signed-out"));
    throw new ApiError(body?.error ?? `HTTP ${res.status}`, res.status, body?.details ?? []);
  }
  return body as T;
}

// ---- Graph -------------------------------------------------------------------------------------

export type Condition = { field: string; op: Op; value: unknown };
export type Op =
  | "eq" | "ne" | "contains" | "not_contains" | "matches"
  | "gt" | "gte" | "lt" | "lte" | "exists" | "not_exists";

export type SourceData = { name: string };
export type FilterData = { name: string; mode: "all" | "any"; conditions: Condition[] };
export type RetryPolicy = {
  maxAttempts: number;
  initialBackoffSecs: number;
  maxBackoffSecs: number;
  giveUpAfterSecs: number;
};
export type HttpData = {
  name: string;
  method: string;
  url: string;
  headers: { name: string; value: string }[];
  body: string;
  timeoutSecs: number;
  retry: RetryPolicy;
  /** Which preset it was made from, for the icon. The server ignores it. */
  preset?: string;
};

export type NodeType = "source" | "filter" | "http";
export type GraphNode = {
  id: string;
  type: NodeType;
  position: { x: number; y: number };
  data: SourceData | FilterData | HttpData;
};
export type GraphEdge = { id: string; source: string; target: string; sourceHandle?: string | null };
export type Graph = { nodes: GraphNode[]; edges: GraphEdge[] };

export type GraphResponse = {
  version: number;
  graph: Graph;
  tokens: Record<string, { prefix: string; createdAt: number }>;
};

// ---- Notifications ----------------------------------------------------------------------------

export type Notification = {
  id: string;
  sourceId: string;
  sourceName: string;
  receivedAt: number;
  title: string | null;
  body: string | null;
  priority: number;
  tags: string[];
  url: string | null;
  data: Record<string, unknown>;
  routing: "received" | "routed" | "unrouted";
  deliveries: { delivered: number; pending: number; failed: number };
};

export type Attempt = {
  startedAt: number;
  durationMs: number;
  ok: boolean;
  httpStatus: number | null;
  error: string | null;
  response: string | null;
};

export type Delivery = {
  id: string;
  targetId: string;
  targetName: string;
  parentId: string | null;
  status: "pending" | "sending" | "delivered" | "failed";
  attempts: number;
  nextAttemptAt: number;
  lastError: string | null;
  createdAt: number;
  finishedAt: number | null;
  tries: Attempt[];
};

export type NotificationDetail = Notification & { deliveryList: Delivery[] };

export type TestOutcome = {
  ok: boolean;
  httpStatus: number | null;
  error: string | null;
  response: string | null;
  durationMs: number;
  request: { method: string; url: string; headers: [string, string][]; body: string } | null;
};

// ---- Helpers ----------------------------------------------------------------------------------

/** An error as one line for the GUI, with the server's details when it sent some. */
export function describe(e: unknown): string {
  if (e instanceof ApiError) return e.details.length ? `${e.message}: ${e.details.join("; ")}` : e.message;
  return e instanceof Error ? e.message : String(e);
}

export function timeAgo(ms: number): string {
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 0) return `in ${formatDuration(-s)}`;
  if (s < 5) return "just now";
  return `${formatDuration(s)} ago`;
}

export function formatDuration(s: number): string {
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.round(s / 60)}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}

export function formatTime(ms: number): string {
  return new Date(ms).toLocaleString(undefined, {
    month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit",
  });
}

/** Calls `onEvent` for every server event, reconnecting by itself. Returns a function that stops it. */
export function subscribe(onEvent: (e: { kind: string; notificationId: string }) => void): () => void {
  const source = new EventSource("/api/events");
  source.onmessage = (m) => {
    try {
      onEvent(JSON.parse(m.data));
    } catch {
      /* ignore a malformed event */
    }
  };
  return () => source.close();
}
