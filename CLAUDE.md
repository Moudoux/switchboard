# Switchboard

A notification server: projects post notifications to one endpoint, and Switchboard stores them in SQLite and delivers them onwards along a routing graph drawn in a React Flow GUI. `README.md` covers the features, API, templates and configuration; read it first. `docs/next-session.md` has the current state and what's next.

- `server/`: Rust (axum, sqlx/SQLite, reqwest with rustls/ring, minijinja, webauthn-rs). One binary, `switchboard`, with `serve`, `enroll` and `health`.
- `web/`: React + TypeScript + Vite + `@xyflow/react`, built into `web/dist` and served by the server.
- `Dockerfile`: multi-arch (amd64, arm64). Rust cross-compiles on the build platform; don't add `RUN` steps to the runtime stage, which would need QEMU.
- `.github/workflows/ci.yml`: tests, then builds and pushes `ghcr.io/<owner>/<repo>`. A push to `main` whose version has no `v` tag yet is released by CI (version image tags, git tag, GitHub Release), so releasing is just bumping `version` in `server/Cargo.toml` and `web/package.json` together (CI fails if they differ). Don't tag by hand. Actions are pinned to commit SHAs with the version in a comment; keep it that way, and run `actionlint` after changes.

## Rules for the design

- **The ingest handler stays fast.** `POST /api/v1/notify` checks the token, validates, inserts one row and answers `202`. Anything slower (routing, rendering, sending) belongs in `server/src/engine/`, which picks the work up from the database.
- **All state that matters is in SQLite**, so a restart carries on: `received` notifications are routed at start, and `sending` deliveries go back to `pending`. Don't keep queues only in memory.
- **The graph's rules exist twice:** `Graph::validate` in `server/src/graph.rs` and `isValidConnection` in `web/src/flow/FlowEditor.tsx`. Change them together.
- **Passkeys only.** No password login. `switchboard enroll` prints its one-time link only to a terminal, and nothing else may print a secret (tokens are shown once in the GUI and stored as SHA-256).
- Graph JSON is stored as the GUI sends it (positions included); the server parses what it needs and ignores the rest.

## Checks

`just check` runs what CI runs: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and the GUI build. `just image` builds the multi-arch image locally on the `rootless` buildx builder.

pnpm here refuses packages published less than 7 days ago (`minimumReleaseAge`). Respect it: pick the newest version old enough rather than excluding packages.

## Testing by hand

Passkeys can't be scripted with curl. For API tests against a throwaway database (in the session scratchpad, never `data/`), insert a session row directly, then send its cookie:

```sh
sqlite3 test.db "INSERT INTO sessions VALUES ('$(printf testsession | sha256sum | cut -d' ' -f1)', 0, 99999999999999)"
curl -H 'Cookie: switchboard_session=testsession' localhost:8080/api/graph
```

The real passkey flow can be tested in headless Chrome over the DevTools protocol with `WebAuthn.addVirtualAuthenticator`; for its enrolment link, insert a known token's hash into `enroll_tokens` instead of printing one.

Start test servers with `setsid -f` so stopping a background shell doesn't stop them, and stop them with `pkill -x switchboard`. Never use `pkill -f` with a pattern that also appears in your own command line: it kills the shell running it. The user runs OpenSnitch, which can silently drop connections from tools it hasn't seen before (bun's fetch hung on this); curl and Node are allowed.

## Deployment

It runs on the Raspberry Pi as a Docker stack behind the nginx proxy, managed by taskpod from `~/Documents/Code/arch-config/pi`. Changes there go through that repo's task files and its `CLAUDE.md`, never by hand. `deploy/` has the compose file and the nginx site. The nginx site must not buffer `/api/events`.
