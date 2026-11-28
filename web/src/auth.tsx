// Sign-in with passkeys or single sign-on (OpenID Connect), and enrolment. There are no passwords.

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

export type SsoPurpose = "signIn" | "link" | "enroll";

/** Goes to the single sign-on provider. The server sends the browser back to the GUI afterwards. */
export async function startSso(purpose: SsoPurpose, enrollToken?: string) {
  const { url } = await api<{ url: string }>("/api/auth/oidc/start", {
    method: "POST",
    json: { purpose, enrollToken },
  });
  location.assign(url);
}

/** The outcome of a single sign-on, which the server puts in `?sso=` when it sends the browser back. */
export function takeSsoResult(): string | undefined {
  const params = new URLSearchParams(location.search);
  const code = params.get("sso");
  if (!code) return undefined;
  params.delete("sso");
  const rest = params.toString();
  history.replaceState(null, "", location.pathname + (rest ? `?${rest}` : "") + location.hash);
  return code;
}

export function describeSso(code: string): { ok: boolean; text: string } {
  switch (code) {
    case "linked":
      return { ok: true, text: "Single sign-on is linked. You can sign in with it from now on." };
    case "unlinked":
      return {
        ok: false,
        text:
          "That single sign-on account isn't linked to Switchboard, so you weren't signed in. Sign in another way and link it under Settings, or use a link from switchboard enroll.",
      };
    case "denied":
      return { ok: false, text: "The single sign-on provider didn't sign you in." };
    case "expired":
      return { ok: false, text: "The single sign-on took too long or was already used. Try again." };
    case "signed-out":
      return { ok: false, text: "You were signed out before the link was made. Sign in and try again." };
    default:
      return { ok: false, text: "Single sign-on didn't work; the server log has the details." };
  }
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

export type AuthStatus = {
  signedIn: boolean;
  hasPasskeys: boolean;
  hasIdentities: boolean;
  passkeyLogin: boolean;
  /** Single sign-on, when the server has a provider set up. */
  sso: { label: string } | null;
};

function SsoButton({ label, purpose, enrollToken, onError }: {
  label: string;
  purpose: SsoPurpose;
  enrollToken?: string;
  onError: (message: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <button
      className="primary wide"
      disabled={busy}
      onClick={async () => {
        setBusy(true);
        try {
          await startSso(purpose, enrollToken);
        } catch (e) {
          onError(describeError(e));
          setBusy(false);
        }
      }}
    >
      {busy ? "Going to sign-in…" : label}
    </button>
  );
}

export function SignIn({ status, ssoResult, onDone }: { status: AuthStatus; ssoResult?: string; onDone: () => void }) {
  const [error, setError] = useState<string | undefined>(ssoResult && describeSso(ssoResult).text);
  const [busy, setBusy] = useState(false);
  const passkeys = status.passkeyLogin && status.hasPasskeys;
  if (!status.hasPasskeys && !status.hasIdentities) {
    return (
      <Shell>
        <p>Nothing is set up to sign in with yet. On the server, run:</p>
        <pre className="code">docker exec -it switchboard switchboard enroll</pre>
        <p className="muted">
          It prints a one-time link to register the first passkey{status.sso ? " or link single sign-on" : ""}.
        </p>
      </Shell>
    );
  }
  return (
    <Shell>
      <p className="muted">Sign in to configure routing and see notifications.</p>
      {status.sso && <SsoButton label={status.sso.label} purpose="signIn" onError={setError} />}
      {passkeys && !browserSupportsWebAuthn() && (
        <p className="error">This browser can't use passkeys. Open Switchboard over HTTPS in a current browser.</p>
      )}
      {passkeys && browserSupportsWebAuthn() && <button
        className={status.sso ? "wide" : "primary wide"}
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
      </button>}
      {!passkeys && !status.sso && (
        <p className="muted">
          Signing in with a passkey is turned off. Run <code>switchboard enroll</code> on the server for a one-time link.
        </p>
      )}
      {error && <p className="error">{error}</p>}
    </Shell>
  );
}

export function Enroll({ token, sso, onDone }: { token: string; sso: AuthStatus["sso"]; onDone: () => void }) {
  const [name, setName] = useState(guessDeviceName());
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  return (
    <Shell>
      {sso && (
        <>
          <p>Link your single sign-on account, so it can sign in to Switchboard. The link works once.</p>
          <SsoButton label={sso.label} purpose="enroll" enrollToken={token} onError={setError} />
          <p className="muted small">Or register a passkey on this device instead:</p>
        </>
      )}
      {!sso && <p>Register a passkey on this device. The link works once.</p>}
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
