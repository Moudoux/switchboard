import { StrictMode, useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { api } from "./api";
import { Enroll, SignIn } from "./auth";
import { FlowEditor } from "./flow/FlowEditor";
import { Notifications } from "./Notifications";
import { Settings } from "./Settings";
import "./styles.css";

type Status = { signedIn: boolean; hasPasskeys: boolean };
type Page = "routing" | "notifications" | "settings";

function currentPage(): Page {
  const p = location.pathname.replace(/^\//, "");
  return p === "notifications" || p === "settings" ? p : "routing";
}

function App() {
  const [status, setStatus] = useState<Status>();
  const [page, setPage] = useState<Page>(currentPage);
  const enrollToken = location.pathname === "/enroll" ? location.hash.slice(1) : "";

  const check = useCallback(() => {
    api<Status>("/api/auth/status").then(setStatus).catch(() => setStatus({ signedIn: false, hasPasskeys: true }));
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
  if (enrollToken) return <Enroll token={enrollToken} onDone={check} />;
  if (!status.signedIn) return <SignIn hasPasskeys={status.hasPasskeys} onDone={check} />;

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
        {page === "settings" && <Settings />}
      </main>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
