// Passkey sign-in and enrolment. There are no passwords.

import { useState } from "react";
import { browserSupportsWebAuthn, startAuthentication, startRegistration } from "@simplewebauthn/browser";
import { api, ApiError } from "./api";

type Options = { ceremony: string; options: { publicKey: any } };

export async function signIn() {
  const { ceremony, options } = await api<Options>("/api/auth/login/start", { method: "POST" });
  const credential = await startAuthentication({ optionsJSON: options.publicKey });
  await api("/api/auth/login/finish", { method: "POST", json: { ceremony, credential } });
}

/** Registers a passkey: signed in, or with the token from an enrolment link. */
export async function registerPasskey(name: string, enrollToken?: string) {
  const { ceremony, options } = await api<Options>("/api/auth/register/start", {
    method: "POST",
    json: { name, enrollToken },
  });
  const credential = await startRegistration({ optionsJSON: options.publicKey });
  await api("/api/auth/register/finish", { method: "POST", json: { ceremony, credential } });
}

export function describeError(e: unknown): string {
  if (e instanceof ApiError) return e.message;
  if (e instanceof Error) {
    if (e.name === "NotAllowedError") return "The passkey prompt was closed or timed out.";
    if (e.name === "InvalidStateError") return "This device already has a passkey for Switchboard.";
    if (e.name === "SecurityError") return "Passkeys need HTTPS with a trusted certificate, or localhost.";
    return e.message;
  }
  return String(e);
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="auth-screen">
      <div className="auth-card">
        <div className="brand brand-large">
          <span className="brand-mark" aria-hidden />
          Switchboard
        </div>
        {children}
      </div>
    </div>
  );
}

export function SignIn({ hasPasskeys, onDone }: { hasPasskeys: boolean; onDone: () => void }) {
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  if (!browserSupportsWebAuthn()) {
    return (
      <Shell>
        <p className="error">This browser can't use passkeys. Open Switchboard over HTTPS in a current browser.</p>
      </Shell>
    );
  }
  if (!hasPasskeys) {
    return (
      <Shell>
        <p>No passkey is registered yet. On the server, run:</p>
        <pre className="code">docker exec -it switchboard switchboard enroll</pre>
        <p className="muted">It prints a one-time link to register the first passkey.</p>
      </Shell>
    );
  }
  return (
    <Shell>
      <p className="muted">Sign in to configure routing and see notifications.</p>
      <button
        className="primary wide"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          setError(undefined);
          try {
            await signIn();
            onDone();
          } catch (e) {
            setError(describeError(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? "Waiting for passkey…" : "Sign in with a passkey"}
      </button>
      {error && <p className="error">{error}</p>}
    </Shell>
  );
}

export function Enroll({ token, onDone }: { token: string; onDone: () => void }) {
  const [name, setName] = useState(guessDeviceName());
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  return (
    <Shell>
      <p>Register a passkey on this device. The link works once.</p>
      <label className="field">
        <span>Name for this passkey</span>
        <input value={name} onChange={(e) => setName(e.target.value)} autoFocus />
      </label>
      <button
        className="primary wide"
        disabled={busy || !name.trim()}
        onClick={async () => {
          setBusy(true);
          setError(undefined);
          try {
            await registerPasskey(name.trim(), token);
            history.replaceState(null, "", "/");
            onDone();
          } catch (e) {
            setError(describeError(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? "Waiting for passkey…" : "Register passkey"}
      </button>
      {error && <p className="error">{error}</p>}
    </Shell>
  );
}

export function guessDeviceName(): string {
  const ua = navigator.userAgent;
  const os = /Android/.test(ua) ? "Android" : /iPhone|iPad/.test(ua) ? "iOS" : /Mac/.test(ua) ? "Mac" : /Linux/.test(ua) ? "Linux" : /Windows/.test(ua) ? "Windows" : "Device";
  const browser = /Firefox\//.test(ua) ? "Firefox" : /Edg\//.test(ua) ? "Edge" : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "";
  return browser ? `${os} · ${browser}` : os;
}
