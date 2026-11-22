// The side panel for the selected node.

import { useState } from "react";
import { api, describe, type FilterData, type HttpData, type SourceData, type TestOutcome } from "../api";
import { fields, ops, parseValue, valueText } from "./conditions";

type Props = {
  id: string;
  type: "source" | "filter" | "http";
  data: SourceData | FilterData | HttpData;
  saved: boolean;
  token?: { prefix: string; createdAt: number };
  onChange: (data: SourceData | FilterData | HttpData) => void;
  onDelete: () => void;
  onTokenCreated: () => void;
};

export function Inspector(props: Props) {
  return (
    <aside className="inspector">
      {props.type === "source" && <SourceForm {...props} data={props.data as SourceData} />}
      {props.type === "filter" && <FilterForm {...props} data={props.data as FilterData} />}
      {props.type === "http" && <HttpForm {...props} data={props.data as HttpData} />}
      <div className="inspector-footer">
        <button className="danger" onClick={props.onDelete}>
          Delete node
        </button>
      </div>
    </aside>
  );
}

function Name({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <label className="field">
      <span>Name</span>
      <input value={value} onChange={(e) => onChange(e.target.value)} />
    </label>
  );
}

// ---- Source -------------------------------------------------------------------------------------

function SourceForm({ id, data, saved, token, onChange, onTokenCreated }: Props & { data: SourceData }) {
  const [newToken, setNewToken] = useState<string>();
  const [error, setError] = useState<string>();
  const example = `curl -X POST ${location.origin}/api/v1/notify \\
  -H "Authorization: Bearer ${newToken ?? "$SWITCHBOARD_TOKEN"}" \\
  -H "Content-Type: application/json" \\
  -d '{"title": "Backup done", "body": "2.1 GB in 4 min", "priority": 3, "tags": ["backup"]}'`;
  return (
    <>
      <h2>Source</h2>
      <p className="muted">Where notifications come in. A project sends to the one endpoint with this source's token.</p>
      <Name value={data.name} onChange={(name) => onChange({ ...data, name })} />
      <div className="field">
        <span>Token</span>
        {token ? (
          <div className="mono">
            {token.prefix}… <span className="muted">made {new Date(token.createdAt).toLocaleDateString()}</span>
          </div>
        ) : (
          <div className="muted">None yet</div>
        )}
        <button
          disabled={!saved}
          title={saved ? "" : "Save the graph first"}
          onClick={async () => {
            if (token && !confirm("Make a new token? The old one stops working at once.")) return;
            try {
              const r = await api<{ token: string }>(`/api/sources/${encodeURIComponent(id)}/token`, { method: "POST" });
              setNewToken(r.token);
              setError(undefined);
              onTokenCreated();
            } catch (e) {
              setError(describe(e));
            }
          }}
        >
          {token ? "Make a new token" : "Make a token"}
        </button>
        {!saved && <div className="muted small">Save the graph before making a token.</div>}
        {error && <div className="error">{error}</div>}
      </div>
      {newToken && (
        <div className="callout">
          <strong>Copy this token now.</strong> It isn't shown again.
          <div className="copy-row">
            <code className="mono break">{newToken}</code>
            <button onClick={() => navigator.clipboard.writeText(newToken)}>Copy</button>
          </div>
        </div>
      )}
      <div className="field">
        <span>Sending a notification</span>
        <pre className="code small">{example}</pre>
        <div className="muted small">
          Only <code>title</code> or <code>body</code> is needed. <code>priority</code> is 1–5 (default 3);{" "}
          <code>url</code> and <code>data</code> (any JSON object) are optional. Plain text works too: send the body as{" "}
          <code>text/plain</code> with optional <code>Title</code>, <code>Priority</code> and <code>Tags</code> headers.
        </div>
      </div>
    </>
  );
}

// ---- Filter ------------------------------------------------------------------------------------

function FilterForm({ data, onChange }: Props & { data: FilterData }) {
  const set = (patch: Partial<FilterData>) => onChange({ ...data, ...patch });
  const setCondition = (i: number, patch: Partial<FilterData["conditions"][number]>) =>
    set({ conditions: data.conditions.map((c, j) => (j === i ? { ...c, ...patch } : c)) });
  return (
    <>
      <h2>Filter</h2>
      <p className="muted">
        Sends a notification on through <span className="tag-match">match</span> when it passes, and through{" "}
        <span className="tag-else">else</span> when it doesn't.
      </p>
      <Name value={data.name} onChange={(name) => set({ name })} />
      <label className="field">
        <span>Passes when</span>
        <select value={data.mode} onChange={(e) => set({ mode: e.target.value as FilterData["mode"] })}>
          <option value="all">all conditions match</option>
          <option value="any">any condition matches</option>
        </select>
      </label>
      <div className="field">
        <span>Conditions</span>
        {data.conditions.map((c, i) => {
          const needsValue = ops.find((o) => o.op === c.op)?.needsValue ?? true;
          return (
            <div className="condition" key={i}>
              <input
                list="condition-fields"
                value={c.field}
                onChange={(e) => setCondition(i, { field: e.target.value })}
                title="title, body, priority, tags, source, url, or data.<key>"
              />
              <select value={c.op} onChange={(e) => setCondition(i, { op: e.target.value as typeof c.op, value: parseValue(valueText(c.value), e.target.value as typeof c.op) })}>
                {ops.map((o) => (
                  <option key={o.op} value={o.op}>
                    {o.label}
                  </option>
                ))}
              </select>
              {needsValue ? (
                <input value={valueText(c.value)} onChange={(e) => setCondition(i, { value: parseValue(e.target.value, c.op) })} />
              ) : (
                <span />
              )}
              <button className="icon" title="Remove" onClick={() => set({ conditions: data.conditions.filter((_, j) => j !== i) })}>
                ×
              </button>
            </div>
          );
        })}
        <datalist id="condition-fields">
          {fields.map((f) => (
            <option key={f} value={f} />
          ))}
          <option value="data." />
        </datalist>
        <button onClick={() => set({ conditions: [...data.conditions, { field: "tags", op: "contains", value: "" }] })}>
          Add condition
        </button>
        <div className="muted small">
          No conditions passes everything. <code>data.printer.bed</code> reads inside the notification's data. Text
          comparisons ignore case; <code>tags contains x</code> means one of the tags is x.
        </div>
      </div>
    </>
  );
}

// ---- HTTP target -------------------------------------------------------------------------------

function HttpForm({ data, onChange }: Props & { data: HttpData }) {
  const set = (patch: Partial<HttpData>) => onChange({ ...data, ...patch });
  const setRetry = (patch: Partial<HttpData["retry"]>) => set({ retry: { ...data.retry, ...patch } });
  const [test, setTest] = useState<TestOutcome | { failed: string }>();
  const [testing, setTesting] = useState(false);
  return (
    <>
      <h2>HTTP target</h2>
      <p className="muted">
        Sends each notification as a request. The URL, header values and body are templates. If it gives up, the
        notification goes on through <span className="tag-fallback">fallback</span>.
      </p>
      <Name value={data.name} onChange={(name) => set({ name })} />
      <div className="row">
        <label className="field" style={{ flex: "0 0 7rem" }}>
          <span>Method</span>
          <select value={data.method} onChange={(e) => set({ method: e.target.value })}>
            {["POST", "PUT", "PATCH", "GET", "DELETE"].map((m) => (
              <option key={m}>{m}</option>
            ))}
          </select>
        </label>
        <label className="field" style={{ flex: 1 }}>
          <span>URL</span>
          <input className="mono" value={data.url} onChange={(e) => set({ url: e.target.value })} />
        </label>
      </div>
      <div className="field">
        <span>Headers</span>
        {data.headers.map((h, i) => (
          <div className="header-row" key={i}>
            <input
              className="mono"
              placeholder="Name"
              value={h.name}
              onChange={(e) => set({ headers: data.headers.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)) })}
            />
            <input
              className="mono"
              placeholder="Value"
              value={h.value}
              onChange={(e) => set({ headers: data.headers.map((x, j) => (j === i ? { ...x, value: e.target.value } : x)) })}
            />
            <button className="icon" title="Remove" onClick={() => set({ headers: data.headers.filter((_, j) => j !== i) })}>
              ×
            </button>
          </div>
        ))}
        <button onClick={() => set({ headers: [...data.headers, { name: "", value: "" }] })}>Add header</button>
      </div>
      <label className="field">
        <span>Body template</span>
        <textarea className="mono" rows={10} spellCheck={false} value={data.body} onChange={(e) => set({ body: e.target.value })} />
        <details className="muted small">
          <summary>Template help</summary>
          <p>
            Jinja syntax. The notification is <code>n</code>: <code>n.title</code>, <code>n.body</code>,{" "}
            <code>n.text</code> (body, or the title if there's none), <code>n.priority</code>, <code>n.tags</code>,{" "}
            <code>n.url</code>, <code>n.data.…</code>, <code>n.source.name</code>, <code>n.receivedAt</code>,{" "}
            <code>n.id</code>.
          </p>
          <p>
            Use <code>{"{{ n.title|tojson }}"}</code> to put text into JSON safely, and{" "}
            <code>{"{{ n|tojson }}"}</code> for everything. A JSON body is sent as <code>application/json</code> unless
            a Content-Type header says otherwise.
          </p>
        </details>
      </label>
      <h3>Retries</h3>
      <div className="grid-2">
        <Num label="Attempts in total" value={data.retry.maxAttempts} min={1} onChange={(v) => setRetry({ maxAttempts: v })} />
        <Num label="Timeout (s)" value={data.timeoutSecs} min={1} onChange={(v) => set({ timeoutSecs: v })} />
        <Num label="First wait (s)" value={data.retry.initialBackoffSecs} min={0} onChange={(v) => setRetry({ initialBackoffSecs: v })} />
        <Num label="Longest wait (s)" value={data.retry.maxBackoffSecs} min={1} onChange={(v) => setRetry({ maxBackoffSecs: v })} />
        <Num label="Give up after (min)" value={Math.round(data.retry.giveUpAfterSecs / 60)} min={1} onChange={(v) => setRetry({ giveUpAfterSecs: v * 60 })} />
      </div>
      <p className="muted small">
        The wait doubles after each failure, up to the longest wait, and honours a Retry-After header. Network errors,
        timeouts, 408, 429 and 5xx are retried; other 4xx answers give up at once.
      </p>
      <h3>Test</h3>
      <button
        disabled={testing}
        onClick={async () => {
          setTesting(true);
          try {
            setTest(await api<TestOutcome>("/api/test", { method: "POST", json: { target: data } }));
          } catch (e) {
            setTest({ failed: describe(e) });
          } finally {
            setTesting(false);
          }
        }}
      >
        {testing ? "Sending…" : "Send a test notification"}
      </button>
      <div className="muted small">Sends one request now with these settings, saved or not. No retries.</div>
      {test && "failed" in test && <div className="error">{test.failed}</div>}
      {test && !("failed" in test) && (
        <div className={`test-result ${test.ok ? "ok" : "bad"}`}>
          <div>
            <strong>{test.ok ? "Delivered" : "Failed"}</strong>
            {test.httpStatus && ` · HTTP ${test.httpStatus}`} · {test.durationMs} ms
          </div>
          {test.error && <div>{test.error}</div>}
          {test.request && (
            <details>
              <summary>Request sent</summary>
              <pre className="code small">
                {test.request.method} {test.request.url}
                {"\n"}
                {test.request.headers.map(([k, v]) => `${k}: ${v}\n`).join("")}
                {"\n"}
                {test.request.body}
              </pre>
            </details>
          )}
          {test.response && (
            <details>
              <summary>Response</summary>
              <pre className="code small">{test.response}</pre>
            </details>
          )}
        </div>
      )}
    </>
  );
}

function Num({ label, value, min, onChange }: { label: string; value: number; min: number; onChange: (v: number) => void }) {
  return (
    <label className="field">
      <span>{label}</span>
      <input type="number" min={min} value={value} onChange={(e) => onChange(Math.max(min, Number(e.target.value) || min))} />
    </label>
  );
}
