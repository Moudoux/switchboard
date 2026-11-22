use tokio::sync::mpsc;

use crate::notification::Notification;
use crate::state::AppState;

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

/// Creates the deliveries for a notification. `again` routes one that was routed before, for redelivery.
pub async fn route(state: &AppState, id: &str, again: bool) -> sqlx::Result<usize> {
    let mut tx = state.db.begin().await?;
    let n: Option<Notification> = sqlx::query_as("SELECT * FROM notifications WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(n) = n else { return Ok(0) };
    let targets = state.graph().route(&n.source_id, &n);
    let created = super::create_deliveries(state, &mut tx, id, &targets, None).await?;
    let status = if created > 0 { "routed" } else { "unrouted" };
    let claimed = sqlx::query(
        "UPDATE notifications SET status = ? WHERE id = ? AND (status = 'received' OR ?)",
    )
    .bind(status)
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
    tracing::debug!(notification = id, targets = created, "routed");
    state.wake.notify_one();
    state.emit("routed", id);
    Ok(created)
}
