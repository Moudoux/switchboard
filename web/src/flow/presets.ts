import type { AppriseData, DedupData, FilterData, QuietHours, RetryPolicy, SourceData, TargetData } from "../api";

export const defaultRetry: RetryPolicy = {
  maxAttempts: 8,
  initialBackoffSecs: 5,
  maxBackoffSecs: 600,
  giveUpAfterSecs: 6 * 3600,
};

/** Off, but ready to switch on: nights in the browser's time zone, urgent ones still through. */
export const defaultQuietHours = (): QuietHours => ({
  enabled: false,
  start: "22:00",
  end: "07:00",
  days: [0, 1, 2, 3, 4, 5, 6],
  timezone: Intl.DateTimeFormat().resolvedOptions().timeZone ?? "",
  action: "hold",
  minPriority: 5,
});

/** An unguessable ntfy topic. crypto.getRandomValues also works outside a secure context. */
function randomTopic(): string {
  const alphabet = "abcdefghijklmnopqrstuvwxyz0123456789";
  const bytes = crypto.getRandomValues(new Uint8Array(20));
  return "switchboard-" + Array.from(bytes, (b) => alphabet[b % alphabet.length]).join("");
}

export type Preset = {
  id: string;
  type: "http" | "apprise";
  label: string;
  description: string;
  make: () => TargetData;
};

// Templates are MiniJinja (Jinja2). The notification is `n`; see the template help in the inspector.
export const presets: Preset[] = [
  {
    id: "webhook",
    type: "http",
    label: "Webhook",
    description: "POST the whole notification as JSON to any URL",
    make: () => ({
      name: "Webhook",
      method: "POST",
      url: "https://",
      headers: [],
      body: "{{ n|tojson }}",
      timeoutSecs: 10,
      retry: { ...defaultRetry },
      quietHours: defaultQuietHours(),
      preset: "webhook",
    }),
  },
  {
    id: "discord",
    type: "http",
    label: "Discord",
    description: "A message with an embed, coloured by priority, through a channel webhook",
    make: () => ({
      name: "Discord",
      method: "POST",
      url: "https://discord.com/api/webhooks/…",
      headers: [],
      body: `{
  "username": "Switchboard",
  {% if n.priority >= 5 %}"content": "@here",{% endif %}
  "embeds": [{
    "title": {{ (n.title or n.source.name)[:256]|tojson }},
    {% if n.body %}"description": {{ n.body[:4000]|tojson }},{% endif %}
    {% if n.url %}"url": {{ n.url|tojson }},{% endif %}
    "color": {{ [9807270, 3447003, 5793266, 15105570, 15548997][n.priority - 1] }},
    "footer": {"text": {{ (n.source.name ~ (" · " ~ n.tags|join(", ") if n.tags else ""))|tojson }}},
    "timestamp": {{ n.receivedAt|tojson }}
  }]
}`,
      timeoutSecs: 10,
      retry: { ...defaultRetry },
      quietHours: defaultQuietHours(),
      preset: "discord",
    }),
  },
  {
    id: "ntfy",
    type: "http",
    label: "ntfy",
    description: "Push to the ntfy app on a phone, through ntfy.sh or your own ntfy server",
    make: () => ({
      name: "ntfy",
      method: "POST",
      // JSON publishing goes to the server's root, with the topic in the body.
      url: "https://ntfy.sh",
      headers: [],
      body: `{# Anyone who knows a topic's name can read it on ntfy.sh, so keep the random one.
   Subscribe to it in the ntfy app. For a reserved topic or your own server with
   access control, add a header "Authorization" with "Bearer tk_…". #}
{
  "topic": ${JSON.stringify(randomTopic())},
  "title": {{ (n.title or n.source.name)|tojson }},
  "message": {{ (n.body or n.title or n.source.name)[:4000]|tojson }},
  "priority": {{ n.priority }},
  {% if n.url %}"click": {{ n.url|tojson }},{% endif %}
  "tags": {{ n.tags|tojson }}
}`,
      timeoutSecs: 10,
      retry: { ...defaultRetry },
      quietHours: defaultQuietHours(),
      preset: "ntfy",
    }),
  },
  {
    id: "claude-monitor",
    type: "http",
    label: "Claude Monitor",
    description: "Show it on the Claude Monitor's 320×240 screen",
    make: () => ({
      name: "Claude Monitor",
      method: "POST",
      // The Pi's containers can't resolve .local names; use the board's IP, shown on its screen.
      url: "http://192.168.1.51/display",
      headers: [],
      body: `{
  "title": {{ (n.title or n.source.name)[:40]|tojson }},
  "color": {{ ["grey", "white", "cyan", "orange", "red"][n.priority - 1]|tojson }},
  "lines": [
    {{ (n.body or "")|tojson }},
    {"text": {{ n.source.name|tojson }}, "size": 1, "color": "grey"}
  ]
}`,
      timeoutSecs: 10,
      retry: { maxAttempts: 5, initialBackoffSecs: 10, maxBackoffSecs: 120, giveUpAfterSecs: 1800 },
      quietHours: defaultQuietHours(),
      preset: "claude-monitor",
    }),
  },
  {
    id: "apprise",
    type: "apprise",
    label: "Apprise",
    description: "Telegram, ntfy, email, Pushover and 100+ more, through an Apprise API server",
    make: (): AppriseData => ({
      name: "Apprise",
      url: "http://apprise:8000",
      urls: [""],
      key: "",
      tag: "",
      title: "{{ n.title or n.source.name }}",
      body: "{{ n.text }}",
      format: "text",
      headers: [],
      timeoutSecs: 30,
      retry: { ...defaultRetry },
      quietHours: defaultQuietHours(),
    }),
  },
];

export const newSource = (): SourceData => ({ name: "New source" });
export const newDedup = (): DedupData => ({
  name: "Dedupe",
  key: "{{ n.source.id }}|{{ n.title }}|{{ n.body }}",
  windowSecs: 600,
});
export const newFilter = (): FilterData => ({
  name: "Filter",
  mode: "all",
  conditions: [{ field: "priority", op: "gte", value: 4 }],
});
