# Sending notifications to Switchboard

This guide is for agents and scripts that need to send notifications to a Switchboard server. You send every notification to one endpoint. Switchboard stores it and routes it to wherever its owner has configured (Discord, a phone through Apprise, webhooks, and so on), so you never need to know where it ends up.

## What you need

Two values, both given to you by the owner of the Switchboard server:

| Variable             | Example                          | What it is                                         |
|----------------------|----------------------------------|----------------------------------------------------|
| `SWITCHBOARD_URL`    | `https://notify.example.lan`     | The server's address, without a trailing slash.    |
| `SWITCHBOARD_TOKEN`  | `sb_3f9a…` (67 characters)       | The token of the **source** you send as.           |

From a container on the same Docker network as Switchboard, `SWITCHBOARD_URL` can be `http://switchboard:8080`.

**The token is a secret.** Read it from the environment, or from a file the owner points you to. Don't print it, log it, commit it, or put it in a command line that ends up in logs. Don't ask a person to paste it into a chat. If you don't have one, ask the owner to make one: in Switchboard's GUI, they add a source, save the graph and click **Make a token**. The token decides which source a notification comes from, and so how it is routed. One source per project or machine is usual.

## Sending one

`POST $SWITCHBOARD_URL/api/v1/notify` with the token as a bearer token and a JSON body:

```sh
curl -fsS -X POST "$SWITCHBOARD_URL/api/v1/notify" \
  -H "Authorization: Bearer $SWITCHBOARD_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title": "Backup done", "body": "2.1 GB in 4 min", "priority": 3, "tags": ["backup"]}'
```

A success is `202 Accepted` with `{"id": "<notification id>"}`. The notification is stored at that point and is delivered afterwards, so a `202` doesn't mean it has reached anyone yet.

### Fields

| Field      | Type             | Required | Notes                                                                |
|------------|------------------|----------|----------------------------------------------------------------------|
| `title`    | string           | one of   | At most 1,000 bytes.                                                 |
| `body`     | string           | these    | At most 32,000 bytes. `message` is accepted as another name for it.  |
| `priority` | integer 1–5      | no       | Default 3. See below.                                                |
| `tags`     | list of strings  | no       | At most 50. Routing filters can match on them.                       |
| `url`      | string           | no       | A link the notification is about, such as a CI run or a dashboard.   |
| `data`     | object           | no       | Anything else, as JSON. Routing filters and templates can read it.   |

A title or a body is required. A blank one doesn't count. **Unknown fields are rejected** with a `400`, so check the spelling. Put anything extra in `data`. The whole request must be under 256 KiB.

### Priority

| Priority | Use it for                                                   |
|----------|--------------------------------------------------------------|
| 1        | Noise you'd only read in the history.                         |
| 2        | Low: routine information.                                     |
| 3        | Normal: a job finished, something worth knowing. The default. |
| 4        | High: something failed or needs attention soon.               |
| 5        | Urgent: something is down or needs a person now.              |

Use 5 sparingly. Quiet hours usually let priority 5 through at night. Some targets alert harder at 5: the Discord preset pings `@here`, the ntfy preset sends it as ntfy's max priority, and Apprise targets send it as a failure.

### Plain text

For a quick message from a shell, send the body as `text/plain` and the other fields as headers (`Title`, `Priority`, `Tags` comma-separated, `Url`):

```sh
echo "nightly backup failed: disk full" | curl -fsS -X POST "$SWITCHBOARD_URL/api/v1/notify" \
  -H "Authorization: Bearer $SWITCHBOARD_TOKEN" -H "Content-Type: text/plain" \
  -H "Title: Backup" -H "Priority: 5" -H "Tags: backup,failed" --data-binary @-
```

The body must be UTF-8. Header values are best kept to ASCII; put anything else in a JSON body.

## Answers and errors

| Status | Body                          | Meaning, and what to do                                                      |
|--------|-------------------------------|------------------------------------------------------------------------------|
| `202`  | `{"id": "…"}`                 | Stored. Done.                                                                |
| `400`  | `{"error": "…"}`              | The notification is invalid; the message says why. Fix it, don't retry as is. |
| `401`  | `{"error": "…"}`              | Missing or unknown token, or its source was removed. Don't retry; tell the owner. |
| `413`  | (plain text)                  | Over 256 KiB. Shorten the body or `data`.                                    |
| `5xx`, network error, timeout | | Retry with a backoff, such as 1, 2, 4, 8 seconds, then give up.          |

A retry after a timeout can store the same notification twice, because the first attempt may have succeeded. If that matters, ask the owner whether the route has a dedupe node. It drops repeats with the same key, by default the source, title and body, within a time window.

Answering takes a few milliseconds, because Switchboard only stores the notification before it answers. A timeout of 10 seconds is plenty. Sending a notification should never hold up or break the work it reports on, so catch failures and carry on.

## Examples

### Python (standard library only)

```python
import json, os, urllib.request

def notify(title, body=None, priority=3, tags=(), url=None, data=None):
    payload = {"title": title, "priority": priority, "tags": list(tags)}
    if body: payload["body"] = body
    if url: payload["url"] = url
    if data: payload["data"] = data
    req = urllib.request.Request(
        os.environ["SWITCHBOARD_URL"] + "/api/v1/notify",
        data=json.dumps(payload).encode(),
        headers={
            "Authorization": "Bearer " + os.environ["SWITCHBOARD_TOKEN"],
            "Content-Type": "application/json",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return json.load(r)["id"]
    except Exception as e:  # never let a notification break the job
        print(f"switchboard: notification not sent: {e}")
        return None
```

### Node.js 18+ / Bun / Deno

```js
async function notify({ title, body, priority = 3, tags = [], url, data }) {
  try {
    const res = await fetch(`${process.env.SWITCHBOARD_URL}/api/v1/notify`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${process.env.SWITCHBOARD_TOKEN}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ title, body, priority, tags, url, data }),
      signal: AbortSignal.timeout(10_000),
    });
    if (!res.ok) console.error("switchboard:", res.status, (await res.json().catch(() => ({}))).error);
    else return (await res.json()).id;
  } catch (e) {
    console.error("switchboard: notification not sent:", e.message);
  }
}
```

`JSON.stringify` drops `undefined` fields, so leaving out `body`, `url` or `data` is fine.

### Shell, with JSON built safely

Don't paste variables into JSON by hand; quotes and newlines break it. Use `jq`:

```sh
jq -n --arg title "Deploy finished" --arg body "$summary" --arg url "$run_url" \
  '{title: $title, body: $body, url: $url, priority: 3, tags: ["deploy"]}' |
  curl -fsS -m 10 -X POST "$SWITCHBOARD_URL/api/v1/notify" \
    -H "Authorization: Bearer $SWITCHBOARD_TOKEN" -H "Content-Type: application/json" -d @- \
  || echo "switchboard: notification not sent" >&2
```

## Writing good notifications

- **Title:** the outcome in a few words: "Backup failed", "Print finished", "Deploy to prod done". Many targets show only the title on a lock screen.
- **Body:** the useful detail: numbers, the error message, what to do next. Keep it short. Discord shows at most 4,000 characters.
- **Same event, same text:** a repeat of the same problem should have the same title and body, so that dedupe recognises it. Put changing details, such as a timestamp or attempt number, in `data`, not in the title.
- **`url`:** link to where a person can act on it.
- **`tags` and `data`:** these are for routing. A tag like `backup` or `ci`, or `data.host`, lets the owner send some notifications somewhere else without you changing anything.
- **Secrets:** never put them in a notification. Notifications are stored (30 days by default) and shown in the GUI and in every target.

## Checking the server

`GET $SWITCHBOARD_URL/healthz` answers `ok` when the server and its database are working. It needs no token. Sending a notification needs no other setup, and there is nothing to read back: what happened to a notification is shown in Switchboard's GUI, which only its owner can open.
