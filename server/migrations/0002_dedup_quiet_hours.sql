-- Deduplication, quiet hours and the target filter in the history.

-- Why routing turned out as it did, for the GUI: a JSON array of {"text": ..., "notificationId": ...}.
-- notifications.status can now also be `duplicate`: a dedupe node stopped it and nothing else matched.
ALTER TABLE notifications ADD COLUMN notes TEXT NOT NULL DEFAULT '[]';

-- When quiet hours held a delivery back, the time they end; retries count their give-up time from it.
-- deliveries.status can now also be `skipped`: quiet hours dropped it.
ALTER TABLE deliveries ADD COLUMN held_until INTEGER;

CREATE INDEX deliveries_target ON deliveries (target_id, notification_id);

-- The last notification that passed a dedupe node with a given key. The key is stored as SHA-256.
CREATE TABLE dedup_keys (
    node_id         TEXT NOT NULL,
    key_hash        TEXT NOT NULL,
    notification_id TEXT NOT NULL,
    passed_at       INTEGER NOT NULL,
    PRIMARY KEY (node_id, key_hash)
);
