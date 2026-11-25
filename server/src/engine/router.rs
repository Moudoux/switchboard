use std::collections::HashMap;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::notification::Notification;
use crate::state::AppState;
use crate::util::{now_ms, sha256_hex};

/// Routes notifications as their ids arrive, after first routing any left over from before a restart.
pub async fn run(state: AppState, mut rx: mpsc::UnboundedReceiver<String>) {
    match sqlx::query_scalar::<_, String>(
        "SELECT id FROM notifications WHERE status = 'received' ORDER BY received_at",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(ids) => {
            if !ids.is_empty() {
                tracing::info!(
                    count = ids.len(),
                    "routing notifications left from before the restart"
                );
            }
            for id in ids {
                route_logged(&state, &id).await;
            }
        }
        Err(e) => tracing::error!("can't read unrouted notifications: {e}"),
    }
    while let Some(id) = rx.recv().await {
        route_logged(&state, &id).await;
    }
}

async fn route_logged(state: &AppState, id: &str) {
    if let Err(e) = route(state, id, false).await {
        // It stays `received` and is routed again at the next start.
        tracing::error!(notification = id, "routing failed: {e}");
    }
}

/// Creates the deliveries for a notification. `again` routes one that was routed before, for
/// redelivery; that is a deliberate send, so dedupe nodes and quiet hours don't stop it.
pub async fn route(state: &AppState, id: &str, again: bool) -> sqlx::Result<usize> {
    let mut tx = state.db.begin().await?;
    let n: Option<Notification> = sqlx::query_as("SELECT * FROM notifications WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(n) = n else { return Ok(0) };
    let graph = state.graph();
    let now = now_ms();

    // What every dedupe node in the graph would say; routing only asks the ones on its way.
    let template_value = n.template_value();
    let mut keys = HashMap::new();
    let mut seen_before = HashMap::new();
    for (node_id, dedup) in graph.dedup_nodes() {
        let key = match crate::template::render(&dedup.key, &template_value) {
            Ok(key) => sha256_hex(&key),
            Err(e) => {
                // Treat it as new rather than lose it.
                tracing::error!(notification = id, node = dedup.name, "dedupe key: {e}");
                continue;
            }
        };
        if !again {
            let previous: Option<(String, i64)> = sqlx::query_as(
                "SELECT notification_id, passed_at FROM dedup_keys WHERE node_id = ? AND key_hash = ? AND passed_at > ?",
            )
            .bind(node_id)
            .bind(&key)
            .bind(now.saturating_sub(dedup.window_secs.min(i64::MAX as u64 / 1000) as i64 * 1000))
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(previous) = previous.filter(|(other, _)| *other != n.id) {
                seen_before.insert(node_id.to_string(), previous);
            }
        }
        keys.insert(node_id.to_string(), key);
    }
    let routing = graph.route(&n.source_id, &n, |node| seen_before.contains_key(node));

    for node_id in &routing.passed {
        if let Some(key) = keys.get(node_id) {
            sqlx::query(
                "INSERT INTO dedup_keys (node_id, key_hash, notification_id, passed_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (node_id, key_hash) DO UPDATE SET notification_id = ?3, passed_at = ?4",
            )
            .bind(node_id)
            .bind(key)
            .bind(&n.id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    }
    let notes: Vec<Value> = routing
        .duplicates
        .iter()
        .filter_map(|node_id| {
            let (original, at) = seen_before.get(node_id)?;
            let name = graph.node(node_id).map_or("", |n| n.kind.name());
            Some(json!({
                "text": format!(
                    "Duplicate at \u{201c}{name}\u{201d} of a notification {} earlier",
                    ago(now - at)
                ),
                "notificationId": original,
            }))
        })
        .collect();

    let priority = (!again).then_some(n.priority);
    let created =
        super::create_deliveries(state, &mut tx, id, &routing.targets, None, priority).await?;
    let status = match (created, routing.duplicates.is_empty()) {
        (1.., _) => "routed",
        (0, false) => "duplicate",
        (0, true) => "unrouted",
    };
    let claimed = sqlx::query(
        "UPDATE notifications SET status = ?, notes = ? WHERE id = ? AND (status = 'received' OR ?)",
    )
    .bind(status)
    .bind(Value::Array(notes).to_string())
    .bind(id)
    .bind(again)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if claimed == 0 {
        // Routed already, by the startup scan or the channel; drop what this pass created.
        tx.rollback().await?;
        return Ok(0);
    }
    tx.commit().await?;
    tracing::debug!(notification = id, targets = created, status, "routed");
    state.wake.notify_one();
    state.emit("routed", id);
    Ok(created)
}

/// A rough length of time for a note, such as "3 min" or "2 h".
fn ago(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        3600..86_400 => format!("{} h", s / 3600),
        _ => format!("{} days", s / 86_400),
    }
}
