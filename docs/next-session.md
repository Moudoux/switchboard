# Next session (updated 2026-11-25)

## State

Published as the public repo https://github.com/Moudoux/switchboard (MIT licence). Commits use the user's GitHub noreply address, set in this repo's git config. CI builds `ghcr.io/moudoux/switchboard` on pushes to `main`.

### Built and verified

- **Server** (`server/`, ~2,000 lines of Rust): ingest, SQLite schema, routing graph with validation, filters, templated HTTP targets, retries with backoff and jitter and `Retry-After`, fallbacks, retention cleanup, passkey-only auth with `enroll`, the GUI's API, SSE live events, and the `health` command. Six unit tests cover routing, conditions, templates, backoff and time formatting.
- **End to end against a scratch database:**
  - **Ingest:** `202` in about 1 ms; 401 without a token, 400 for an invalid body.
  - **Retries and fallback:** a dead target retried 3 times, gave up, and its fallback was delivered. Rendered URL, header and body reached a local receiver.
  - **Crash recovery:** the server was killed with SIGKILL during a retry, and the delivery was sent after restart.
  - **Presets:** the Discord, Claude Monitor and webhook presets render valid JSON through `/api/test`.
  - **Passkeys:** register through the enrolment link, sign out, sign in, all in headless Chrome with a virtual authenticator. A used link is rejected.
- **GUI** (`web/`): routing editor, notification history with deliveries and attempts, and passkey settings. Screenshots were checked in light and dark mode.
- **Image:** builds for amd64 and arm64 (63 MB and 73 MB). The amd64 container became healthy and served the GUI, and the arm64 binary ran under QEMU.
- **CI:** `just check` passes locally, and `actionlint` is clean. The workflow hasn't run on GitHub yet.

### Version 0.2.0 (2026-11-25, not committed yet)

- **Dedupe node** (`dedup` in the graph): key template, window, `new` and `duplicate` outputs. Keys are stored as SHA-256 in `dedup_keys` and cleaned up hourly. A notification that only a dedupe node stopped gets status `duplicate`, and a note in `notifications.notes` with the original's id.
- **Quiet hours** on every target (`quietHours`, in `server/src/quiet_hours.rs`): start, end, days, IANA zone (jiff 1.0 with the tz database bundled), hold or drop, and a minimum priority that still goes through. A held delivery is `pending` with `held_until`, and its give-up time counts from then. A dropped delivery is `skipped`. Manual **Send again** and **Retry** skip both dedupe and quiet hours.
- **Apprise target** (`apprise` in the graph): talks to an Apprise API server, stateless (`/notify/` with `urls`) or stateful (`/notify/<key>` with `tag`). Priority sets the type: 4 is warning, 5 is failure. A 424 is retried. Taken from the apprise-api README: 424 means a service failed *or* no tag matched.
- **History filter by target:** `GET /api/targets` (targets in the graph plus targets that are only in the history), and `?target=` on `/api/notifications`. With a target set, the status counts and filters cover only that target's deliveries. New status filters: held, skipped, duplicate.
- **Migration** `0002_dedup_quiet_hours.sql`. **Tests:** 13, including a handler-level test (`api::admin::tests`) that runs the router and the list handler against a scratch database.
- **Not verified:** the GUI in a browser, and a real Apprise server. Loopback TCP on the Arch machine timed out on 2026-11-25, even for `python -m http.server` plus curl outside the sandbox, so neither a test server nor headless Chrome could be reached. It was likely OpenSnitch or a firewall rule. `just check` passes, including the typed GUI build.

## Next steps, in order

0. **Check 0.2.0 by hand once loopback works:** add a dedupe node and an Apprise target in the GUI, turn on quiet hours, and look at the inspector, the node cards and the history filter in light and dark mode. Run an Apprise API container (`caronc/apprise`) and use **Send a test notification** with an `ntfy://` URL.

1. **Test with Discord as the receiver.** For now this is the only target the user wants to test. Run Switchboard (locally with `just server` + `just web`, or on the Pi), then:
   - Add a Discord target from the preset, and paste the channel's webhook URL in the GUI. The user pastes it; it's a credential, so don't ask for it in chat.
   - Use **Send a test notification**, then post real notifications with a source token.
   - Check the priority colours, `@here` at priority 5, long bodies (cut at 4000 characters), and a 429 from Discord's rate limit (it should honour `Retry-After`).
2. **Deploy on the Pi** through `arch-config/pi`; propose the changes and let the user apply them:
   - A `switchboard` task: `~/stacks/switchboard/compose.yaml` from `deploy/compose.yaml`, `container_name: switchboard` on the external `nginx` network, and `SWITCHBOARD_ORIGIN=https://notify.devops.lan`.
   - A proxy template `notify.conf.template` from `deploy/nginx.conf.template`, plus its `include` line in `nginx.conf`.
   - DNS for `notify.devops.lan` if AdGuard doesn't already have a wildcard rewrite.
   - **Check that the user's browsers trust the `*.devops.lan` certificate.** Passkeys refuse certificate errors. If they don't trust it, use the Tailscale name (`notify.<tailnet>.ts.net`) as the origin.
   - Then `docker exec -it switchboard switchboard enroll`. The user runs this; it prints a one-time link.
   - The Claude Monitor target needs the board's IP, because containers can't resolve `claude-monitor.local`.

**Not planned now:** a Surface RT viewer. The design only has to stay open to more kinds of receivers: Claude Monitor, Discord, the Surface RT later. A possible later shape is a "viewer channel" node that viewer clients subscribe to over SSE with a viewer token, which also works behind NAT.

## Ideas not built yet

- Rate limiting, or grouping repeats into one digest (dedupe only drops or diverts them).
- Re-checking quiet hours on retries (today they apply only when a delivery is created).
- Other target types: exec, MQTT, SMTP (Apprise covers many of these through its server).
- An Apprise API service in `deploy/compose.yaml`.
- A secrets store for templates (`{{ secrets.discord_url }}`). Today a Discord webhook URL, which is itself a credential, is stored in the graph JSON and shown in the GUI.
- Delivery counts on the graph's target nodes. `HttpNode` already draws `data.stats`, but nothing fills it.
- Graph version history and rollback in the GUI. Every version is already stored in `graph_versions`.
- API tokens for scripting the admin API.
- Integration tests for the API handlers.

## Decisions made

- **When the ack is sent:** the ingest endpoint answers after one SQLite insert (WAL, `synchronous=NORMAL`), not before. That takes about a millisecond, and the alternative would lose acknowledged notifications on a crash. The user asked for an immediate ack; this keeps it immediate in practice while staying durable.
- **Senders:** one endpoint for every sender. The source is identified by its token.
- **Auth:** passkeys only (webauthn-rs, pure Rust, no OpenSSL), with a single admin user.
- **Dependencies:** React Flow 12.12.1, because 13.x was younger than pnpm's 7-day `minimumReleaseAge`.
