//! Everything that happens after a notification has been stored and acknowledged.
//!
//! 1. The router reads the stored notification, follows the graph from its source, and creates one
//!    `pending` delivery per target it reaches.
//! 2. The delivery worker sends due deliveries, records each attempt, and either finishes them,
//!    schedules a retry with backoff, or gives up and creates deliveries for the fallback targets.
//!
//! All state is in SQLite, so a restart carries on where it stopped: notifications still
//! `received` are routed again, and deliveries left `sending` are sent again.

pub mod delivery;
pub mod router;
pub mod send;

use crate::quiet_hours::Verdict;
use crate::state::AppState;
use crate::util::now_ms;

/// Creates a pending delivery to each target for a notification, skipping targets it already has
/// one for when `parent_id` is set (so fallbacks that point at each other don't loop).
///
/// With `priority`, the targets' quiet hours apply: a delivery is held until they end, or created
/// already `skipped`. Without it (a manual send or retry), it is sent now.
pub async fn create_deliveries(
    state: &AppState,
    tx: &mut sqlx::SqliteConnection,
    notification_id: &str,
    targets: &[String],
    parent_id: Option<&str>,
    priority: Option<i64>,
) -> sqlx::Result<usize> {
    let graph = state.graph();
    let now = now_ms();
    let mut created = 0;
    for target_id in targets {
        let Some(target) = graph.target(target_id) else {
            continue;
        };
        if parent_id.is_some() {
            let existing: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM deliveries WHERE notification_id = ? AND target_id = ?",
            )
            .bind(notification_id)
            .bind(target_id)
            .fetch_one(&mut *tx)
            .await?;
            if existing > 0 {
                continue;
            }
        }
        let quiet = target.quiet_hours();
        let verdict = match priority.map(|p| quiet.verdict(p, now)) {
            None => Verdict::Send,
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                tracing::error!(target = target.name(), "quiet hours ignored: {e}");
                Verdict::Send
            }
        };
        let (status, held_until, note, finished_at) = match verdict {
            Verdict::Send => ("pending", None, None, None),
            Verdict::Hold(until) => (
                "pending",
                Some(until),
                Some(format!(
                    "held for quiet hours {}–{}",
                    quiet.start, quiet.end
                )),
                None,
            ),
            Verdict::Drop => (
                "skipped",
                None,
                Some(format!(
                    "dropped in quiet hours {}–{}",
                    quiet.start, quiet.end
                )),
                Some(now),
            ),
        };
        sqlx::query(
            "INSERT INTO deliveries (id, notification_id, target_id, target_name, parent_id, status, next_attempt_at, held_until, last_error, created_at, updated_at, finished_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(notification_id)
        .bind(target_id)
        .bind(target.name())
        .bind(parent_id)
        .bind(status)
        .bind(held_until.unwrap_or(now))
        .bind(held_until)
        .bind(note)
        .bind(now)
        .bind(now)
        .bind(finished_at)
        .execute(&mut *tx)
        .await?;
        created += 1;
    }
    Ok(created)
}

/// Deletes notifications past the retention period, expired sessions, and dedupe keys whose window
/// has passed or whose node is gone, once an hour.
pub async fn cleanup(state: AppState) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        tick.tick().await;
        let cutoff = now_ms() - state.config.retention_days * 86_400_000;
        let result = sqlx::query(
            "DELETE FROM notifications WHERE received_at < ?1 AND NOT EXISTS
               (SELECT 1 FROM deliveries d WHERE d.notification_id = notifications.id AND d.status IN ('pending', 'sending'))",
        )
        .bind(cutoff)
        .execute(&state.db)
        .await;
        match result {
            Ok(r) if r.rows_affected() > 0 => {
                tracing::info!(deleted = r.rows_affected(), "removed old notifications")
            }
            Ok(_) => {}
            Err(e) => tracing::error!("cleanup failed: {e}"),
        }
        let _ = sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
            .bind(now_ms())
            .execute(&state.db)
            .await;
        if let Err(e) = cleanup_dedup(&state).await {
            tracing::error!("dedupe cleanup failed: {e}");
        }
    }
}

async fn cleanup_dedup(state: &AppState) -> sqlx::Result<()> {
    let graph = state.graph();
    let windows: Vec<(String, i64)> = graph
        .dedup_nodes()
        .map(|(id, d)| {
            (
                id.to_string(),
                d.window_secs.min(i64::MAX as u64 / 1000) as i64,
            )
        })
        .collect();
    let mut tx = state.db.begin().await?;
    let nodes: Vec<String> = sqlx::query_scalar("SELECT DISTINCT node_id FROM dedup_keys")
        .fetch_all(&mut *tx)
        .await?;
    for node in nodes {
        match windows.iter().find(|(id, _)| *id == node) {
            Some((_, window)) => {
                sqlx::query("DELETE FROM dedup_keys WHERE node_id = ? AND passed_at < ?")
                    .bind(&node)
                    .bind(now_ms().saturating_sub(window.saturating_mul(1000)))
                    .execute(&mut *tx)
                    .await?;
            }
            None => {
                sqlx::query("DELETE FROM dedup_keys WHERE node_id = ?")
                    .bind(&node)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await
}
