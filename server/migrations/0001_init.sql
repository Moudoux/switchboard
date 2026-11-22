-- All timestamps are Unix milliseconds (UTC).

-- Key/value settings, such as the WebAuthn user id.
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Every saved version of the routing graph. The newest row is the live one.
CREATE TABLE graph_versions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at INTEGER NOT NULL,
    graph      TEXT NOT NULL
);

-- One token per source node. Only the SHA-256 of the token is stored.
CREATE TABLE source_tokens (
    node_id    TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    prefix     TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- Passkeys that can sign in to the GUI. `passkey` is webauthn-rs's Passkey as JSON (public key only).
CREATE TABLE passkeys (
    id           TEXT PRIMARY KEY,   -- credential id, base64url
    name         TEXT NOT NULL,
    passkey      TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER
);

-- One-time links made by `notifyd enroll` to register a passkey without being signed in.
CREATE TABLE enroll_tokens (
    token_hash TEXT PRIMARY KEY,
    expires_at INTEGER NOT NULL
);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

-- status: received (stored, not routed yet), routed (deliveries created), unrouted (no target matched)
CREATE TABLE notifications (
    id          TEXT PRIMARY KEY,
    source_id   TEXT NOT NULL,
    source_name TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    title       TEXT,
    body        TEXT,
    priority    INTEGER NOT NULL,
    tags        TEXT NOT NULL,   -- JSON array of strings
    url         TEXT,
    data        TEXT NOT NULL,   -- JSON object
    status      TEXT NOT NULL
);
CREATE INDEX notifications_received ON notifications (received_at DESC);
CREATE INDEX notifications_status ON notifications (status);

-- One delivery of a notification to one target. A fallback delivery points at the delivery that failed.
-- status: pending, sending, delivered, failed
CREATE TABLE deliveries (
    id              TEXT PRIMARY KEY,
    notification_id TEXT NOT NULL REFERENCES notifications (id) ON DELETE CASCADE,
    target_id       TEXT NOT NULL,
    target_name     TEXT NOT NULL,
    parent_id       TEXT REFERENCES deliveries (id) ON DELETE CASCADE,
    status          TEXT NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    finished_at     INTEGER
);
CREATE INDEX deliveries_due ON deliveries (status, next_attempt_at);
CREATE INDEX deliveries_notification ON deliveries (notification_id);

CREATE TABLE attempts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    delivery_id TEXT NOT NULL REFERENCES deliveries (id) ON DELETE CASCADE,
    started_at  INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    ok          INTEGER NOT NULL,
    http_status INTEGER,
    error       TEXT,
    response    TEXT
);
CREATE INDEX attempts_delivery ON attempts (delivery_id);
