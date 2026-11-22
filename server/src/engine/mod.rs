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

use crate::state::AppState;
use crate::util::now_ms;

/// Creates a pending delivery to each target for a notification, skipping targets it already has
/// one for when `parent_id` is set (so fallbacks that point at each other don't loop).
pub async fn create_deliveries(
    state: &AppState,
    tx: &mut sqlx::SqliteConnection,
    notification_id: &str,
    targets: &[String],
    parent_id: Option<&str>,
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
        sqlx::query(
            "INSERT INTO deliveries (id, notification_id, target_id, target_name, parent_id, status, next_attempt_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'pending', ?, ?, ?)",
        )
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(notification_id)
        .bind(target_id)
        .bind(&target.name)
        .bind(parent_id)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        created += 1;
    }
    Ok(created)
}

/// Deletes notifications past the retention period, and expired sessions, once an hour.
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
    }
}
