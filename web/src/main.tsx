import { StrictMode, useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { api } from "./api";
import { type AuthStatus, Enroll, SignIn, takeSsoResult } from "./auth";
import { FlowEditor } from "./flow/FlowEditor";
import { Notifications } from "./Notifications";
import { Settings } from "./Settings";
import "./styles.css";

type Status = AuthStatus;
type Page = "routing" | "notifications" | "settings";

function currentPage(): Page {
  const p = location.pathname.replace(/^\//, "");
  return p === "notifications" || p === "settings" ? p : "routing";
}

// Read once, at load: the server's verdict on a single sign-on it just sent the browser back from.
const initialSsoResult = takeSsoResult();

function App() {
  const [status, setStatus] = useState<Status>();
  const [page, setPage] = useState<Page>(currentPage);
  const enrollToken = location.pathname === "/enroll" ? location.hash.slice(1) : "";
  const ssoResult = initialSsoResult;

  const check = useCallback(() => {
    api<Status>("/api/auth/status")
      .then(setStatus)
      .catch(() => setStatus({ signedIn: false, hasPasskeys: true, hasIdentities: false, passkeyLogin: true, sso: null }));
  }, []);
  useEffect(check, [check]);
  useEffect(() => {
    const out = () => setStatus((s) => (s ? { ...s, signedIn: false } : s));
    const pop = () => setPage(currentPage());
    window.addEventListener("signed-out", out);
    window.addEventListener("popstate", pop);
    return () => {
      window.removeEventListener("signed-out", out);
      window.removeEventListener("popstate", pop);
    };
  }, []);

  if (!status) return null;
  if (enrollToken) return <Enroll token={enrollToken} sso={status.sso} onDone={check} />;
  if (!status.signedIn) return <SignIn status={status} ssoResult={ssoResult} onDone={check} />;

  const go = (p: Page) => {
    history.pushState(null, "", p === "routing" ? "/" : `/${p}`);
    setPage(p);
  };
  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark" aria-hidden />
          Switchboard
        </div>
        <nav>
          {(["routing", "notifications", "settings"] as Page[]).map((p) => (
            <button key={p} className={`tab ${page === p ? "active" : ""}`} onClick={() => go(p)}>
              {p[0].toUpperCase() + p.slice(1)}
            </button>
          ))}
        </nav>
        <div className="spacer" />
        <button
          className="ghost"
          onClick={async () => {
            await api("/api/auth/logout", { method: "POST" });
            check();
          }}
        >
          Sign out
        </button>
      </header>
      <main>
        {page === "routing" && <FlowEditor />}
        {page === "notifications" && <Notifications />}
        {page === "settings" && <Settings sso={status.sso} ssoResult={ssoResult} />}
      </main>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
