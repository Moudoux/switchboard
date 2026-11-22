import type { FilterData, HttpData, RetryPolicy, SourceData } from "../api";

export const defaultRetry: RetryPolicy = {
  maxAttempts: 8,
  initialBackoffSecs: 5,
  maxBackoffSecs: 600,
  giveUpAfterSecs: 6 * 3600,
};

export type Preset = {
  id: string;
  label: string;
  description: string;
  make: () => HttpData;
};

// Templates are MiniJinja (Jinja2). The notification is `n`; see the template help in the inspector.
export const presets: Preset[] = [
  {
    id: "webhook",
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
      preset: "webhook",
    }),
  },
  {
    id: "discord",
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
      preset: "discord",
    }),
  },
  {
    id: "claude-monitor",
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
      preset: "claude-monitor",
    }),
  },
];

export const newSource = (): SourceData => ({ name: "New source" });
export const newFilter = (): FilterData => ({
  name: "Filter",
  mode: "all",
  conditions: [{ field: "priority", op: "gte", value: 4 }],
});
