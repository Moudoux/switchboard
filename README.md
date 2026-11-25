# Switchboard

A small, self-hosted notification server. Every project you run sends its notifications to one endpoint, and Switchboard stores them and routes them onwards: to webhooks, Discord, a status display, or anything else that takes an HTTP request. You wire up the routing as a graph in a web GUI.

A sender only ever needs one URL and one token. Changing where notifications end up never touches the senders.

```
 backup script ─┐                       ┌─▶ Discord #alerts ──(fallback)──▶ status display
 3D printer ────┼─▶ Switchboard ─ filter┤
 home server ───┘    (stores it,        └─▶ archive webhook
                      answers at once)
```

## Features

- **One ingest endpoint:** `POST /api/v1/notify` with JSON, or plain text for quick shell use. Each source has its own token.
- **Answers at once:** a notification is stored in SQLite and acknowledged with `202 Accepted` in about a millisecond. Routing and delivery happen afterwards, so a sender is never held up by a slow or broken target.
- **Routing graph:** draw sources, filters and targets in the GUI with [React Flow](https://reactflow.dev). Filters route on priority, tags, title, body, source or anything in the notification's `data`, with a `match` and an `else` output.
- **Deduplication:** a dedupe node lets a notification through once and sends repeats with the same key down a `duplicate` branch, or nowhere, for a time window.
- **Quiet hours:** each target can hold its notifications until morning, or drop them, with urgent ones still going through. Times follow the target's time zone and daylight saving.
- **Apprise:** a native target for an [Apprise API](https://github.com/caronc/apprise-api) server reaches Telegram, ntfy, email, Pushover, Matrix and the 100+ other services Apprise supports.
- **Flexible HTTP targets:** the URL, headers and body are [Jinja templates](https://docs.rs/minijinja), so one target type covers webhooks, Discord, ntfy, Home Assistant, Gotify, Slack and most other services. Presets for a generic webhook, Discord and the Claude Monitor display are built in.
- **Retries and fallbacks:** each target retries with exponential backoff and jitter, honours `Retry-After`, and gives up after a number of attempts or a length of time. When a target gives up, the notification goes on to its fallback targets.
- **Survives restarts:** everything is in SQLite. After a crash or restart, unrouted notifications are routed and interrupted deliveries are sent again.
- **History:** every notification, delivery and attempt, with HTTP status, error and response, live in the GUI. Filter it by target, status or text. Redeliver a notification or retry a single delivery with one click.
- **Passkey-only sign-in:** the GUI has no passwords. The first passkey is registered through a one-time link printed in the server's terminal.
- **Small:** one Rust binary that also serves the GUI, and a multi-arch image for `linux/amd64` and `linux/arm64` (Raspberry Pi).

## Running it

```sh
docker run -d --name switchboard -p 8080:8080 -v ./data:/data \
  -e SWITCHBOARD_ORIGIN=https://notify.example.lan \
  ghcr.io/moudoux/switchboard:latest
```

Passkeys only work over HTTPS with a certificate the browser trusts, or on `http://localhost`. Put Switchboard behind your reverse proxy and set `SWITCHBOARD_ORIGIN` to the address you open it at. [deploy/](deploy/) has a compose file and an nginx site.

Then register your first passkey:

```sh
docker exec -it switchboard switchboard enroll
```

It prints a link that works once, within 15 minutes. Open it, register a passkey, and you're signed in. Add more passkeys later under **Settings**, or run `enroll` again to set one up on another device. The command only runs in a terminal, so the link never ends up in a log.

## Sending notifications

In the GUI, add a **source** for each project, save, and make a token for it. Then:

```sh
curl -X POST https://notify.example.lan/api/v1/notify \
  -H "Authorization: Bearer $SWITCHBOARD_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title": "Backup done", "body": "2.1 GB in 4 min", "priority": 3, "tags": ["backup"]}'
```

| Field      | Type            | Notes                                                       |
|------------|-----------------|-------------------------------------------------------------|
| `title`    | string          | A title or a body is required (`message` works for `body`). |
| `body`     | string          |                                                             |
| `priority` | 1–5             | 1 is lowest, 5 is urgent. Default 3.                        |
| `tags`     | list of strings |                                                             |
| `url`      | string          | A link the notification is about.                           |
| `data`     | object          | Anything else, for filters and templates.                   |

[SENDING-NOTIFICATIONS.md](SENDING-NOTIFICATIONS.md) is a self-contained guide to sending, with examples in Python, Node and shell, written so that an agent or a script's author can follow it without reading anything else.

Unknown fields are rejected, so typos show up straight away. The answer is `202 {"id": "..."}`, or `400`/`401` with `{"error": "..."}`.

For shell scripts, plain text works too:

```sh
echo "nightly backup failed" | curl -X POST https://notify.example.lan/api/v1/notify \
  -H "Authorization: Bearer $SWITCHBOARD_TOKEN" -H "Content-Type: text/plain" \
  -H "Title: Backup" -H "Priority: 5" -H "Tags: backup,failed" --data-binary @-
```

## Routing

- **Source:** where notifications come in. Connect it to filters or targets.
- **Filter:** a list of conditions, all or any of which must match. A notification leaves through `match` when it passes and `else` when it doesn't. Fields are `title`, `body`, `priority`, `tags`, `source`, `url` and `data.<path>`, such as `data.printer.bed`. Text comparisons ignore case, and `tags contains x` means one of the tags is `x`.
- **Dedupe:** works out a key for each notification from a template (by default the source, title and body). The first notification with a key leaves through `new`. One with the same key leaves through `duplicate` while the last one that went through `new` is less than the window old. Leave `duplicate` unconnected to drop repeats. What a dedupe node has seen is kept in the database, so it survives restarts. A notification that only a dedupe node stopped is shown as a **duplicate** in the history, with a link to the original.
- **HTTP target:** sends a request. Connect its bottom `fallback` handle to other targets to try them once it gives up.
- **Apprise target:** sends to an Apprise API server, either to a list of [Apprise URLs](https://github.com/caronc/apprise/wiki) (`POST /notify/`) or to a configuration saved on the server by its key, with an optional tag (`POST /notify/<key>`). The title and body are templates. Priority 4 is sent as Apprise's `warning`, 5 as `failure`, and the rest as `info`. Apprise answers `424` when a service failed or no URL matched the tag, and that is retried. A retry notifies every URL again, so give a service its own target when duplicates matter.

A notification reaching the same target by two paths is delivered to it once.

### Quiet hours

Every target can have quiet hours: a daily period, such as 22:00 to 07:00, on the days you pick, in an IANA time zone (the GUI fills in your browser's; empty uses the server's `TZ`). A period whose end isn't after its start runs past midnight, and belongs to the day it starts on. During quiet hours a new delivery is either **held** until they end or **dropped** (shown as *skipped*). Notifications of priority 5, or 4 and 5, can still go through. A held delivery's give-up time counts from the end of quiet hours. Quiet hours are checked when a delivery is created, and fallback targets check their own. **Send again** and **Retry** in the history send at once, ignoring dedupe nodes and quiet hours. Saving the graph applies it to new notifications at once; deliveries already queued keep their target but use its current settings.

### Templates

The URL, header values and body of a target are templates. The notification is `n`:

| Variable                         | Value                                     |
|----------------------------------|-------------------------------------------|
| `n.title`, `n.body`              | text, or none                             |
| `n.text`                         | the body, or the title if there's no body |
| `n.priority`                     | 1–5                                       |
| `n.tags`                         | list                                      |
| `n.url`                          | text, or none                             |
| `n.data`                         | the `data` object; missing keys are empty |
| `n.source.name`, `n.source.id`   | the source                                |
| `n.receivedAt`, `n.receivedAtMs` | RFC 3339 time, and Unix milliseconds      |
| `n.id`                           | the notification's id                     |

Use `|tojson` to put values into JSON safely: `{"text": {{ n.text|tojson }}}`. `{{ n|tojson }}` sends everything. A body that is valid JSON is sent as `application/json` unless a `Content-Type` header says otherwise. The **Send a test notification** button in the GUI shows exactly what a target sends and what came back.

### Retries

A failed attempt is retried when it was a network error, a timeout, or HTTP 408, 425, 429 or 5xx. Other 4xx answers, such as a wrong URL or a bad token, give up at once. The wait starts at the first wait, doubles after each failure up to the longest wait, varies by ±20%, and is never shorter than the target's `Retry-After`. The delivery gives up after the number of attempts, or when the next attempt would come after the give-up time, whichever comes first. Its fallbacks then start.

## Configuration

| Variable                     | Default                 | Meaning                                                                 |
|------------------------------|-------------------------|-------------------------------------------------------------------------|
| `SWITCHBOARD_ORIGIN`         | `http://localhost:8080` | The address the GUI is opened at. Passkeys are bound to its host.       |
| `SWITCHBOARD_LISTEN`         | `0.0.0.0:8080`          | Address and port to listen on.                                          |
| `SWITCHBOARD_DB`             | `data/switchboard.db`   | The SQLite database (`/data/switchboard.db` in the image).              |
| `SWITCHBOARD_WEB_DIR`        | `web/dist`              | The built GUI (`/app/web` in the image).                                |
| `SWITCHBOARD_RETENTION_DAYS` | `30`                    | Notifications older than this are deleted, unless still being delivered. |
| `SWITCHBOARD_CONCURRENCY`    | `16`                    | Deliveries sent at the same time.                                       |
| `SWITCHBOARD_COOKIE_SECURE`  | `true`                  | Set `false` only to use the GUI over plain HTTP on a hostname other than localhost. Passkeys won't work there anyway. |
| `TZ`                         | UTC                     | The time zone for quiet hours that don't name one, such as `Europe/Stockholm`. |
| `RUST_LOG`                   | `switchboard=info`      | Log level.                                                              |

## Development

Needs Rust (see `server/rust-toolchain.toml`), Node 24 and pnpm. With [just](https://github.com/casey/just):

```sh
just server   # the API on :8080, with SWITCHBOARD_ORIGIN=http://localhost:5173
just web      # the GUI on :5173, proxying /api to the server
just check    # formatting, clippy, tests and the GUI build
just image    # the multi-arch image, locally
```

Open http://localhost:5173, then run `just enroll` for the first passkey's link.

| Path                         | What it is                                                    |
|------------------------------|---------------------------------------------------------------|
| `server/src/api/ingest.rs`   | The ingest endpoint                                           |
| `server/src/graph.rs`        | The graph's types, validation, filters and routing            |
| `server/src/engine/`         | The router with dedupe, the delivery worker with retries and fallbacks, and HTTP and Apprise sending |
| `server/src/quiet_hours.rs`  | Quiet hours                                                   |
| `server/src/api/auth.rs`     | Passkeys, sessions and `enroll`                               |
| `server/src/api/admin.rs`    | The GUI's API                                                 |
| `server/migrations/`         | The SQLite schema                                             |
| `web/src/flow/`              | The routing editor                                            |
| `web/src/Notifications.tsx`  | The notification history                                      |

Pushes to `main` publish `ghcr.io/moudoux/switchboard:latest`. When a push to `main` passes CI and its version (in `server/Cargo.toml` and `web/package.json`) has no tag yet, CI releases it. The same image is also tagged `1.2.0` and `1.2`, and a `v1.2.0` tag and GitHub Release are created. To release, bump the version in both files.

## License

[MIT](LICENSE)
