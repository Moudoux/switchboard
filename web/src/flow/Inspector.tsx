// The side panel for the selected node.

import { useState } from "react";
import {
  api,
  describe,
  formatDuration,
  type AppriseData,
  type DedupData,
  type FilterData,
  type GroupData,
  type HttpData,
  type NodeData,
  type NodeType,
  type QuietHours,
  type SourceData,
  type TargetData,
  type TestOutcome,
} from "../api";
import { fields, ops, parseValue, valueText } from "./conditions";
import { defaultQuietHours } from "./presets";

type Props = {
  id: string;
  type: NodeType;
  data: NodeData;
  saved: boolean;
  token?: { prefix: string; createdAt: number };
  onChange: (data: NodeData) => void;
  onDelete: () => void;
  onTokenCreated: () => void;
};

export function Inspector(props: Props) {
  return (
    <aside className="inspector">
      {props.type === "source" && <SourceForm {...props} data={props.data as SourceData} />}
      {props.type === "filter" && <FilterForm {...props} data={props.data as FilterData} />}
      {props.type === "dedup" && <DedupForm {...props} data={props.data as DedupData} />}
      {props.type === "group" && <GroupForm {...props} data={props.data as GroupData} />}
      {props.type === "http" && <HttpForm {...props} data={props.data as HttpData} />}
      {props.type === "apprise" && <AppriseForm {...props} data={props.data as AppriseData} />}
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
      <TargetCommon type="http" data={data} onChange={onChange} />
    </>
  );
}

// ---- Dedupe ------------------------------------------------------------------------------------

function DedupForm({ data, onChange }: Props & { data: DedupData }) {
  const set = (patch: Partial<DedupData>) => onChange({ ...data, ...patch });
  return (
    <>
      <h2>Dedupe</h2>
      <p className="muted">
        Sends a notification on through <span className="tag-match">new</span> the first time its key is seen, and
        through <span className="tag-else">duplicate</span> while the same key passed less than the window ago. Leave{" "}
        <span className="tag-else">duplicate</span> unconnected to drop repeats.
      </p>
      <Name value={data.name} onChange={(name) => set({ name })} />
      <label className="field">
        <span>Key template</span>
        <input className="mono" value={data.key} onChange={(e) => set({ key: e.target.value })} />
        <div className="muted small">
          Notifications with the same key are duplicates. The default is the source, title and body; use{" "}
          <code>{"{{ n.source.id }}|{{ n.data.job }}"}</code> to group by something in the data.
        </div>
      </label>
      <div className="grid-2">
        <Num label="Window (min)" value={Math.max(1, Math.round(data.windowSecs / 60))} min={1} onChange={(v) => set({ windowSecs: v * 60 })} />
      </div>
      <p className="muted small">
        A repeat is let through again once {formatDuration(data.windowSecs)} have passed since the last one that went
        through. "Send again" in the history ignores dedupe.
      </p>
    </>
  );
}

// ---- Group -------------------------------------------------------------------------------------

function GroupForm({ data, onChange }: Props & { data: GroupData }) {
  return (
    <>
      <h2>Group</h2>
      <p className="muted">
        Sends everything that reaches it on to every target and group it connects to, so sources and filters need one
        edge to reach them all. A target's <span className="tag-fallback">fallback</span> can lead to a group too. A
        notification reaching the same target by two paths is still delivered to it once.
      </p>
      <Name value={data.name} onChange={(name) => onChange({ ...data, name })} />
    </>
  );
}

// ---- Apprise target ----------------------------------------------------------------------------

function AppriseForm({ data, onChange }: Props & { data: AppriseData }) {
  const set = (patch: Partial<AppriseData>) => onChange({ ...data, ...patch });
  const stateful = data.key.trim() !== "" || data.urls.length === 0;
  return (
    <>
      <h2>Apprise target</h2>
      <p className="muted">
        Sends through an{" "}
        <a href="https://github.com/caronc/apprise-api" target="_blank" rel="noreferrer">
          Apprise API
        </a>{" "}
        server to any service Apprise supports. Priority 4 is sent as a warning and 5 as a failure; the rest as info.
        If it gives up, the notification goes on through <span className="tag-fallback">fallback</span>.
      </p>
      <Name value={data.name} onChange={(name) => set({ name })} />
      <label className="field">
        <span>Apprise API server</span>
        <input className="mono" value={data.url} onChange={(e) => set({ url: e.target.value })} placeholder="http://apprise:8000" />
      </label>
      <label className="field">
        <span>Send to</span>
        <select
          value={stateful ? "key" : "urls"}
          onChange={(e) => (e.target.value === "key" ? set({ urls: [], key: data.key || "apprise" }) : set({ urls: [""], key: "", tag: "" }))}
        >
          <option value="urls">Apprise URLs listed here</option>
          <option value="key">A configuration saved on the server</option>
        </select>
      </label>
      {stateful ? (
        <div className="row">
          <label className="field" style={{ flex: 1 }}>
            <span>Configuration key</span>
            <input className="mono" value={data.key} onChange={(e) => set({ key: e.target.value })} />
          </label>
          <label className="field" style={{ flex: 1 }}>
            <span>Tag</span>
            <input className="mono" value={data.tag} placeholder="untagged URLs" onChange={(e) => set({ tag: e.target.value })} />
          </label>
        </div>
      ) : (
        <div className="field">
          <span>Apprise URLs</span>
          {data.urls.map((u, i) => (
            <div className="header-row" key={i}>
              <input
                className="mono"
                placeholder="tgram://bottoken/chatid"
                value={u}
                onChange={(e) => set({ urls: data.urls.map((x, j) => (j === i ? e.target.value : x)) })}
              />
              <button className="icon" title="Remove" onClick={() => set({ urls: data.urls.filter((_, j) => j !== i) })}>
                ×
              </button>
            </div>
          ))}
          <button onClick={() => set({ urls: [...data.urls, ""] })}>Add URL</button>
          <div className="muted small">
            One URL per service, such as <code>ntfy://topic</code> or <code>mailto://user:pass@example.com</code>. A
            retry notifies every URL again, so give a service its own target if duplicates matter.
          </div>
        </div>
      )}
      {stateful && (
        <div className="muted small">
          A comma between tags means or, a space means and; <code>all</code> notifies every URL in the configuration.
        </div>
      )}
      <label className="field">
        <span>Title template</span>
        <input className="mono" value={data.title} onChange={(e) => set({ title: e.target.value })} />
      </label>
      <label className="field">
        <span>Body template</span>
        <textarea className="mono" rows={4} spellCheck={false} value={data.body} onChange={(e) => set({ body: e.target.value })} />
      </label>
      <label className="field">
        <span>Format</span>
        <select value={data.format} onChange={(e) => set({ format: e.target.value as AppriseData["format"] })}>
          <option value="text">Text</option>
          <option value="markdown">Markdown</option>
          <option value="html">HTML</option>
        </select>
      </label>
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
        <div className="muted small">Only needed when the Apprise server sits behind a proxy that wants a login.</div>
      </div>
      <TargetCommon type="apprise" data={data} onChange={onChange} />
    </>
  );
}

// ---- What every target has: retries, quiet hours and a test --------------------------------------

function TargetCommon({ type, data, onChange }: { type: "http" | "apprise"; data: TargetData; onChange: (d: TargetData) => void }) {
  const set = (patch: Partial<TargetData>) => onChange({ ...data, ...patch } as TargetData);
  const setRetry = (patch: Partial<TargetData["retry"]>) => set({ retry: { ...data.retry, ...patch } });
  const quiet = data.quietHours ?? defaultQuietHours();
  const setQuiet = (patch: Partial<QuietHours>) => set({ quietHours: { ...quiet, ...patch } });
  const [test, setTest] = useState<TestOutcome | { failed: string }>();
  const [testing, setTesting] = useState(false);
  return (
    <>
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
        timeouts, 408, 429 and 5xx are retried{type === "apprise" ? ", and so is Apprise's 424" : ""}; other 4xx answers
        give up at once.
      </p>
      <h3>Quiet hours</h3>
      <label className="check">
        <input type="checkbox" checked={quiet.enabled} onChange={(e) => setQuiet({ enabled: e.target.checked })} />
        Keep quiet at set times
      </label>
      {quiet.enabled && (
        <>
          <div className="grid-2">
            <label className="field">
              <span>From</span>
              <input type="time" value={quiet.start} onChange={(e) => setQuiet({ start: e.target.value })} />
            </label>
            <label className="field">
              <span>Until</span>
              <input type="time" value={quiet.end} onChange={(e) => setQuiet({ end: e.target.value })} />
            </label>
          </div>
          <div className="field">
            <span>Starting on</span>
            <div className="days">
              {["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].map((label, day) => (
                <button
                  key={day}
                  className={quiet.days.includes(day) ? "day on" : "day"}
                  onClick={() =>
                    setQuiet({
                      days: quiet.days.includes(day) ? quiet.days.filter((d) => d !== day) : [...quiet.days, day].sort(),
                    })
                  }
                >
                  {label}
                </button>
              ))}
            </div>
          </div>
          <label className="field">
            <span>Time zone</span>
            <input className="mono" value={quiet.timezone} placeholder="the server's" onChange={(e) => setQuiet({ timezone: e.target.value })} />
          </label>
          <div className="grid-2">
            <label className="field">
              <span>Meanwhile</span>
              <select value={quiet.action} onChange={(e) => setQuiet({ action: e.target.value as QuietHours["action"] })}>
                <option value="hold">hold them until the end</option>
                <option value="drop">drop them</option>
              </select>
            </label>
            <label className="field">
              <span>Still send</span>
              <select
                value={quiet.minPriority ?? ""}
                onChange={(e) => setQuiet({ minPriority: e.target.value ? Number(e.target.value) : null })}
              >
                <option value="5">priority 5</option>
                <option value="4">priority 4 and 5</option>
                <option value="">nothing</option>
              </select>
            </label>
          </div>
          <p className="muted small">
            An end before the start runs past midnight. Held notifications are sent when quiet hours end, and their
            give-up time counts from then. Fallbacks have their own quiet hours.
          </p>
        </>
      )}
      <h3>Test</h3>
      <button
        disabled={testing}
        onClick={async () => {
          setTesting(true);
          try {
            setTest(await api<TestOutcome>("/api/test", { method: "POST", json: { type, target: data } }));
          } catch (e) {
            setTest({ failed: describe(e) });
          } finally {
            setTesting(false);
          }
        }}
      >
        {testing ? "Sending…" : "Send a test notification"}
      </button>
      <div className="muted small">Sends one request now with these settings, saved or not. No retries, no quiet hours.</div>
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
