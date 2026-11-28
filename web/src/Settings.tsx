import { useCallback, useEffect, useState } from "react";
import { api, describe, formatTime } from "./api";
import { type AuthStatus, describeError, describeSso, guessDeviceName, registerPasskey, startSso } from "./auth";

type Passkey = { id: string; name: string; createdAt: number; lastUsedAt: number | null };
type Identity = {
  id: number;
  issuer: string;
  subject: string;
  label: string;
  createdAt: number;
  lastUsedAt: number | null;
  /** From the provider that's set up now; others can't sign in. */
  active: boolean;
};

export function Settings({ sso, ssoResult }: { sso: AuthStatus["sso"]; ssoResult?: string }) {
  const [keys, setKeys] = useState<Passkey[]>([]);
  const [identities, setIdentities] = useState<Identity[]>([]);
  const [name, setName] = useState(guessDeviceName());
  const result = ssoResult ? describeSso(ssoResult) : undefined;
  const [error, setError] = useState<string | undefined>(result && !result.ok ? result.text : undefined);
  const [busy, setBusy] = useState(false);
  const load = useCallback(() => {
    api<Passkey[]>("/api/passkeys").then(setKeys).catch((e) => setError(describe(e)));
    api<Identity[]>("/api/sso/identities").then(setIdentities).catch((e) => setError(describe(e)));
  }, []);
  useEffect(load, [load]);

  return (
    <div className="settings">
      {(sso || identities.length > 0) && (
        <>
          <h2>Single sign-on</h2>
          <p className="muted">
            Only an account linked here can sign in. It's recognised by the provider and its user id, not by email.
          </p>
          {result?.ok && <p>{result.text}</p>}
          {identities.length > 0 && (
            <table className="table">
              <thead>
                <tr>
                  <th>Account</th>
                  <th>Provider</th>
                  <th>Linked</th>
                  <th>Last used</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {identities.map((i) => (
                  <tr key={i.id}>
                    <td title={`sub: ${i.subject}`}>{i.label}</td>
                    <td>
                      {i.issuer}
                      {!i.active && <span className="muted"> (not set up now)</span>}
                    </td>
                    <td>{formatTime(i.createdAt)}</td>
                    <td>{i.lastUsedAt ? formatTime(i.lastUsedAt) : "never"}</td>
                    <td>
                      <button
                        className="danger small-btn"
                        onClick={async () => {
                          if (!confirm(`Unlink "${i.label}"? It won't be able to sign in any more.`)) return;
                          setError(undefined);
                          await api(`/api/sso/identities/${i.id}`, { method: "DELETE" }).catch((e) => setError(describe(e)));
                          load();
                        }}
                      >
                        Unlink
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {sso && (
            <div className="row">
              <button
                className="primary"
                onClick={() => startSso("link").catch((e) => setError(describeError(e)))}
              >
                Link single sign-on
              </button>
            </div>
          )}
        </>
      )}
      <h2>Passkeys</h2>
      <p className="muted">Signing in to Switchboard needs a passkey or a linked single sign-on account. There are no passwords.</p>
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
                  onClick={async () => {
                    if (!confirm(`Remove the passkey "${k.name}"?`)) return;
                    setError(undefined);
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
        <code>switchboard enroll</code> on the server and open its link on that device. The same link can
        also link single sign-on.
      </p>
      {error && <p className="error">{error}</p>}
    </div>
  );
}
