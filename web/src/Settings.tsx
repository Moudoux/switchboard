import { useCallback, useEffect, useState } from "react";
import { api, describe, formatTime } from "./api";
import { describeError, guessDeviceName, registerPasskey } from "./auth";

type Passkey = { id: string; name: string; createdAt: number; lastUsedAt: number | null };

export function Settings() {
  const [keys, setKeys] = useState<Passkey[]>([]);
  const [name, setName] = useState(guessDeviceName());
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const load = useCallback(() => {
    api<Passkey[]>("/api/passkeys").then(setKeys).catch((e) => setError(describe(e)));
  }, []);
  useEffect(load, [load]);

  return (
    <div className="settings">
      <h2>Passkeys</h2>
      <p className="muted">Signing in to Switchboard needs one of these. There are no passwords.</p>
      <table className="table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Added</th>
            <th>Last used</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {keys.map((k) => (
            <tr key={k.id}>
              <td>{k.name}</td>
              <td>{formatTime(k.createdAt)}</td>
              <td>{k.lastUsedAt ? formatTime(k.lastUsedAt) : "never"}</td>
              <td>
                <button
                  className="danger small-btn"
                  disabled={keys.length <= 1}
                  title={keys.length <= 1 ? "Add another passkey before removing the last one" : ""}
                  onClick={async () => {
                    if (!confirm(`Remove the passkey "${k.name}"?`)) return;
                    await api(`/api/passkeys/${encodeURIComponent(k.id)}`, { method: "DELETE" }).catch((e) => setError(describe(e)));
                    load();
                  }}
                >
                  Remove
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <h3>Add a passkey</h3>
      <div className="row">
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Name, such as the device" />
        <button
          className="primary"
          disabled={busy || !name.trim()}
          onClick={async () => {
            setBusy(true);
            setError(undefined);
            try {
              await registerPasskey(name.trim());
              load();
            } catch (e) {
              setError(describeError(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          {busy ? "Waiting for passkey…" : "Add passkey"}
        </button>
      </div>
      <p className="muted small">
        To add a passkey on a phone or another computer without signing in there, run{" "}
        <code>switchboard enroll</code> on the server and open its link on that device.
      </p>
      {error && <p className="error">{error}</p>}
    </div>
  );
}
