// Incoming notifications, live, with each one's deliveries and attempts.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  describe,
  formatTime,
  subscribe,
  timeAgo,
  type Delivery,
  type Notification,
  type NotificationDetail,
  type TargetOption,
} from "./api";

const PAGE = 50;

function summary(n: Notification): { label: string; kind: string } {
  if (n.routing === "received") return { label: "routing", kind: "pending" };
  if (n.routing === "unrouted") return { label: "no route", kind: "muted" };
  if (n.routing === "duplicate") return { label: "duplicate", kind: "muted" };
  const { delivered, pending, held, skipped, failed } = n.deliveries;
  if (pending > held) return { label: failed ? "retrying" : "sending", kind: "pending" };
  if (held > 0) return { label: "held", kind: "held" };
  if (failed > 0) return { label: delivered ? "partly failed" : "failed", kind: "bad" };
  if (delivered === 0 && skipped > 0) return { label: "skipped", kind: "muted" };
  return { label: "delivered", kind: "ok" };
}

function deliveryLabel(d: Delivery): string {
  if (d.status === "pending" && d.attempts === 0 && d.heldUntil && d.heldUntil > Date.now())
    return `held until ${formatTime(d.heldUntil)}`;
  if (d.status === "pending" && d.attempts > 0) return `retry ${d.attempts + 1} ${timeAgo(d.nextAttemptAt)}`;
  if (d.status === "pending") return "queued";
  return d.status;
}

const pillKind: Record<Delivery["status"], string> = {
  delivered: "ok",
  failed: "bad",
  skipped: "muted",
  pending: "pending",
  sending: "pending",
};

export function Notifications() {
  const [items, setItems] = useState<Notification[]>([]);
  const [more, setMore] = useState(false);
  const [status, setStatus] = useState("");
  const [target, setTarget] = useState("");
  const [targets, setTargets] = useState<TargetOption[]>([]);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string>();
  const [error, setError] = useState<string>();
  const [tick, setTick] = useState(0);
  const params = useRef({ status, query, target });
  params.current = { status, query, target };

  useEffect(() => {
    api<TargetOption[]>("/api/targets")
      .then(setTargets)
      .catch(() => {});
  }, []);

  const fetchPage = useCallback(async (before?: number) => {
    const q = new URLSearchParams({ limit: String(PAGE) });
    if (before) q.set("before", String(before));
    if (params.current.status) q.set("status", params.current.status);
    if (params.current.target) q.set("target", params.current.target);
    if (params.current.query.trim()) q.set("q", params.current.query.trim());
    return api<Notification[]>(`/api/notifications?${q}`);
  }, []);

  const reload = useCallback(async () => {
    try {
      const page = await fetchPage();
      setItems(page);
      setMore(page.length === PAGE);
      setError(undefined);
    } catch (e) {
      setError(describe(e));
    }
  }, [fetchPage]);

  useEffect(() => {
    const t = setTimeout(reload, 200);
    return () => clearTimeout(t);
  }, [status, query, target, reload]);

  // Live updates: refresh at most twice a second while events arrive.
  useEffect(() => {
    let pending: ReturnType<typeof setTimeout> | undefined;
    const stop = subscribe(() => {
      if (pending) return;
      pending = setTimeout(() => {
        pending = undefined;
        reload();
        setTick((t) => t + 1);
      }, 500);
    });
    return () => {
      stop();
      if (pending) clearTimeout(pending);
    };
  }, [reload]);

  return (
    <div className="split">
      <section className="list-pane">
        <div className="list-filters">
          <input placeholder="Search title and body" value={query} onChange={(e) => setQuery(e.target.value)} />
          <select value={target} onChange={(e) => setTarget(e.target.value)} title="Only notifications sent to this target">
            <option value="">Every target</option>
            {targets.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}
                {t.inGraph ? "" : " (removed)"}
              </option>
            ))}
          </select>
          <select value={status} onChange={(e) => setStatus(e.target.value)}>
            <option value="">All</option>
            <option value="delivered">Delivered</option>
            <option value="pending">Sending or retrying</option>
            <option value="held">Held for quiet hours</option>
            <option value="failed">Failed</option>
            <option value="skipped">Skipped in quiet hours</option>
            <option value="unrouted">No route</option>
            <option value="duplicate">Duplicates</option>
          </select>
        </div>
        {target && (
          <div className="muted small pad-x">
            Showing what reached {targets.find((t) => t.id === target)?.name ?? "this target"}; statuses are its own.
          </div>
        )}
        {error && <div className="error pad">{error}</div>}
        {items.length === 0 && !error && (
          <div className="muted pad">
            Nothing yet. Notifications appear here as soon as a source sends one.
          </div>
        )}
        <ul className="notification-list">
          {items.map((n) => {
            const s = summary(n);
            return (
              <li key={n.id} className={selected === n.id ? "active" : ""} onClick={() => setSelected(n.id)}>
                <div className="n-top">
                  <span className={`prio prio-${n.priority}`} title={`priority ${n.priority}`} />
                  <span className="n-title">{n.title ?? n.body}</span>
                  <span className={`pill ${s.kind}`}>{s.label}</span>
                </div>
                {n.title && n.body && <div className="n-body">{n.body}</div>}
                <div className="n-meta">
                  {n.sourceName} · <span title={formatTime(n.receivedAt)}>{timeAgo(n.receivedAt)}</span>
                  {n.tags.length > 0 && <> · {n.tags.join(", ")}</>}
                </div>
              </li>
            );
          })}
        </ul>
        {more && (
          <button
            className="wide"
            onClick={async () => {
              const page = await fetchPage(items[items.length - 1]?.receivedAt);
              setItems((xs) => [...xs, ...page]);
              setMore(page.length === PAGE);
            }}
          >
            Load older
          </button>
        )}
      </section>
      <section className="detail-pane">
        {selected ? (
          <Detail id={selected} tick={tick} onSelect={setSelected} />
        ) : (
          <div className="muted pad">Select a notification to see where it went.</div>
        )}
      </section>
    </div>
  );
}

function Detail({ id, tick, onSelect }: { id: string; tick: number; onSelect: (id: string) => void }) {
  const [n, setN] = useState<NotificationDetail>();
  const [error, setError] = useState<string>();
  const load = useCallback(() => {
    api<NotificationDetail>(`/api/notifications/${id}`)
      .then((d) => {
        setN(d);
        setError(undefined);
      })
      .catch((e) => setError(describe(e)));
  }, [id]);
  useEffect(load, [load, tick]);

  if (error) return <div className="error pad">{error}</div>;
  if (!n) return null;

  const roots = n.deliveryList.filter((d) => !d.parentId);
  const children = (d: Delivery) => n.deliveryList.filter((c) => c.parentId === d.id);

  return (
    <div className="detail">
      <div className="detail-head">
        <span className={`prio prio-${n.priority}`} />
        <h2>{n.title ?? "(no title)"}</h2>
      </div>
      <div className="muted">
        From <strong>{n.sourceName}</strong> · {formatTime(n.receivedAt)} · priority {n.priority}
        {n.tags.length > 0 && <> · tags {n.tags.join(", ")}</>}
      </div>
      {n.body && <p className="detail-body">{n.body}</p>}
      {n.url && (
        <p>
          <a href={n.url} target="_blank" rel="noreferrer">
            {n.url}
          </a>
        </p>
      )}
      {Object.keys(n.data).length > 0 && (
        <details>
          <summary>Data</summary>
          <pre className="code small">{JSON.stringify(n.data, null, 2)}</pre>
        </details>
      )}
      <div className="detail-section">
        <h3>Deliveries</h3>
        <button
          onClick={async () => {
            if (!confirm("Route this notification again through the current graph? Every matching target gets it again.")) return;
            await api(`/api/notifications/${n.id}/redeliver`, { method: "POST" }).catch((e) => alert(describe(e)));
            load();
          }}
        >
          Send again
        </button>
      </div>
      {n.routing === "unrouted" && <p className="muted">No target matched when it arrived.</p>}
      {n.notes.map((note, i) => (
        <p key={i} className="muted">
          {note.text}
          {note.notificationId && (
            <>
              {" "}
              <button className="link" onClick={() => onSelect(note.notificationId!)}>
                Show the original
              </button>
            </>
          )}
        </p>
      ))}
      {n.routing === "received" && <p className="muted">Waiting to be routed.</p>}
      <div className="deliveries">
        {roots.map((d) => (
          <DeliveryCard key={d.id} d={d} children={children} onChange={load} />
        ))}
      </div>
    </div>
  );
}

function DeliveryCard({ d, children, onChange }: { d: Delivery; children: (d: Delivery) => Delivery[]; onChange: () => void }) {
  const kids = children(d);
  const label = deliveryLabel(d);
  return (
    <div className="delivery">
      <div className={`delivery-card ${d.status}`}>
        <div className="delivery-head">
          <strong>{d.targetName}</strong>
          <span className={`pill ${label.startsWith("held") ? "held" : pillKind[d.status]}`}>{label}</span>
          <span className="spacer" />
          {(d.status === "failed" || d.status === "delivered" || d.status === "skipped") && (
            <button
              className="small-btn"
              onClick={async () => {
                await api(`/api/deliveries/${d.id}/retry`, { method: "POST" }).catch((e) => alert(describe(e)));
                onChange();
              }}
            >
              {d.status === "skipped" ? "Send now" : "Retry"}
            </button>
          )}
        </div>
        {d.lastError && d.status !== "delivered" && (
          <div className={`small ${d.status === "skipped" || (d.heldUntil && d.attempts === 0) ? "muted" : "error"}`}>{d.lastError}</div>
        )}
        {d.tries.length > 0 && (
          <ol className="attempts">
            {d.tries.map((a, i) => (
              <li key={i} className={a.ok ? "ok" : "bad"}>
                <span>{formatTime(a.startedAt)}</span>
                <span>{a.ok ? "✓" : "✗"}</span>
                <span>{a.httpStatus ? `HTTP ${a.httpStatus}` : (a.error ?? "")}</span>
                <span className="muted">{a.durationMs} ms</span>
                {a.response && !a.ok && <span className="attempt-response mono">{a.response}</span>}
              </li>
            ))}
          </ol>
        )}
      </div>
      {kids.length > 0 && (
        <div className="fallbacks">
          <div className="fallback-label">fallback</div>
          {kids.map((k) => (
            <DeliveryCard key={k.id} d={k} children={children} onChange={onChange} />
          ))}
        </div>
      )}
    </div>
  );
}
