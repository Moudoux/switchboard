use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::send;
use crate::graph::RetryPolicy;
use crate::notification::Notification;
use crate::state::AppState;
use crate::util::now_ms;

#[derive(sqlx::FromRow)]
struct Delivery {
    id: String,
    notification_id: String,
    target_id: String,
    attempts: i64,
    created_at: i64,
    /// When quiet hours held it back, the time they ended; the give-up time counts from then.
    held_until: Option<i64>,
}

/// The delivery worker: sends due deliveries, at most `concurrency` at once.
pub async fn run(state: AppState) {
    // Deliveries that were being sent when the server stopped are sent again.
    if let Err(e) = sqlx::query("UPDATE deliveries SET status = 'pending' WHERE status = 'sending'")
        .execute(&state.db)
        .await
    {
        tracing::error!("can't reset interrupted deliveries: {e}");
    }
    let slots = Arc::new(Semaphore::new(state.config.concurrency));
    loop {
        let free = slots.available_permits();
        if free == 0 {
            state.wake.notified().await; // a finished attempt wakes us
            continue;
        }
        match claim(&state, free).await {
            Ok(ids) => {
                let full = ids.len() == free;
                for id in ids {
                    let permit = slots.clone().acquire_owned().await.expect("semaphore open");
                    tokio::spawn(attempt(state.clone(), id, permit));
                }
                if full {
                    continue; // there may be more due
                }
            }
            Err(e) => {
                tracing::error!("can't read due deliveries: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }
        let wait = match next_due(&state).await {
            Ok(Some(at)) => Duration::from_millis((at - now_ms()).clamp(0, 30_000) as u64),
            _ => Duration::from_secs(30),
        };
        tokio::select! {
            _ = state.wake.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

async fn claim(state: &AppState, limit: usize) -> sqlx::Result<Vec<String>> {
    let now = now_ms();
    sqlx::query_scalar(
        "UPDATE deliveries SET status = 'sending', updated_at = ?1 WHERE id IN
           (SELECT id FROM deliveries WHERE status = 'pending' AND next_attempt_at <= ?1 ORDER BY next_attempt_at LIMIT ?2)
         RETURNING id",
    )
    .bind(now)
    .bind(limit as i64)
    .fetch_all(&state.db)
    .await
}

async fn next_due(state: &AppState) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar("SELECT MIN(next_attempt_at) FROM deliveries WHERE status = 'pending'")
        .fetch_one(&state.db)
        .await
}

async fn attempt(state: AppState, id: String, _permit: OwnedSemaphorePermit) {
    if let Err(e) = attempt_inner(&state, &id).await {
        tracing::error!(delivery = id, "delivery attempt failed to record: {e}");
        // Put it back so it isn't stuck in `sending`; try again in a minute.
        let _ = sqlx::query("UPDATE deliveries SET status = 'pending', next_attempt_at = ? WHERE id = ? AND status = 'sending'")
            .bind(now_ms() + 60_000)
            .bind(&id)
            .execute(&state.db)
            .await;
    }
    drop(_permit);
    state.wake.notify_one();
}

async fn attempt_inner(state: &AppState, id: &str) -> sqlx::Result<()> {
    let d: Delivery = sqlx::query_as("SELECT * FROM deliveries WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    let n: Notification = sqlx::query_as("SELECT * FROM notifications WHERE id = ?")
        .bind(&d.notification_id)
        .fetch_one(&state.db)
        .await?;
    let graph = state.graph();
    let started_at = now_ms();

    let Some(target) = graph.target(&d.target_id) else {
        return finish(
            state,
            &d,
            "failed",
            Some("the target was removed from the graph"),
            &[],
            None,
        )
        .await;
    };
    let outcome = send::send(&state.http, target, &n.template_value()).await;
    let retry = target.retry();

    sqlx::query(
        "INSERT INTO attempts (delivery_id, started_at, duration_ms, ok, http_status, error, response) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(started_at)
    .bind(outcome.duration_ms)
    .bind(outcome.ok)
    .bind(outcome.http_status.map(i64::from))
    .bind(&outcome.error)
    .bind(&outcome.response)
    .execute(&state.db)
    .await?;

    let attempts = d.attempts + 1;
    if outcome.ok {
        tracing::info!(
            notification = d.notification_id,
            target = target.name(),
            attempts,
            "delivered"
        );
        return finish(
            state,
            &Delivery { attempts, ..d },
            "delivered",
            None,
            &[],
            None,
        )
        .await;
    }
    let error = outcome.error.as_deref().unwrap_or("failed");
    let now = now_ms();
    if outcome.retryable
        && let Some(delay) = retry_delay(retry, attempts, outcome.retry_after)
        && now + delay as i64
            <= d.held_until.unwrap_or(d.created_at) + retry.give_up_after_secs as i64 * 1000
    {
        tracing::warn!(
            notification = d.notification_id,
            target = target.name(),
            attempts,
            error,
            "retrying in {}s",
            delay / 1000
        );
        sqlx::query(
            "UPDATE deliveries SET status = 'pending', attempts = ?, next_attempt_at = ?, last_error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(attempts)
        .bind(now + delay as i64)
        .bind(error)
        .bind(now)
        .bind(id)
        .execute(&state.db)
        .await?;
        state.emit("delivery", &d.notification_id);
        return Ok(());
    }
    let fallbacks = graph.fallbacks(&d.target_id);
    tracing::warn!(
        notification = d.notification_id,
        target = target.name(),
        attempts,
        error,
        fallbacks = fallbacks.len(),
        "gave up"
    );
    finish(
        state,
        &Delivery { attempts, ..d },
        "failed",
        Some(error),
        &fallbacks,
        Some(n.priority),
    )
    .await
}

/// Marks a delivery done and creates its fallback deliveries, in one transaction. The fallbacks'
/// quiet hours apply for a notification of `priority`.
async fn finish(
    state: &AppState,
    d: &Delivery,
    status: &str,
    error: Option<&str>,
    fallbacks: &[String],
    priority: Option<i64>,
) -> sqlx::Result<()> {
    let now = now_ms();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE deliveries SET status = ?, attempts = ?, last_error = COALESCE(?, last_error), updated_at = ?, finished_at = ? WHERE id = ?",
    )
    .bind(status)
    .bind(d.attempts)
    .bind(error)
    .bind(now)
    .bind(now)
    .bind(&d.id)
    .execute(&mut *tx)
    .await?;
    super::create_deliveries(
        state,
        &mut tx,
        &d.notification_id,
        fallbacks,
        Some(&d.id),
        priority,
    )
    .await?;
    tx.commit().await?;
    state.emit("delivery", &d.notification_id);
    Ok(())
}

/// Milliseconds until the next attempt after `attempts` attempts, or None when they're used up.
/// Exponential backoff with ±20 % jitter, at least as long as the server's Retry-After.
fn retry_delay(policy: &RetryPolicy, attempts: i64, retry_after: Option<Duration>) -> Option<u64> {
    if attempts >= i64::from(policy.max_attempts) {
        return None;
    }
    let exp = (attempts - 1).clamp(0, 30) as u32;
    let base = policy
        .initial_backoff_secs
        .saturating_mul(1 << exp)
        .min(policy.max_backoff_secs.max(1)) as f64
        * 1000.0;
    let jittered = base * (0.8 + 0.4 * rand::random::<f64>());
    let floor = retry_after.map_or(0.0, |d| d.as_millis() as f64);
    Some(jittered.max(floor) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_stops() {
        let p = RetryPolicy {
            max_attempts: 4,
            initial_backoff_secs: 10,
            max_backoff_secs: 30,
            give_up_after_secs: 3600,
        };
        let d1 = retry_delay(&p, 1, None).unwrap();
        assert!((8_000..=12_000).contains(&d1));
        let d3 = retry_delay(&p, 3, None).unwrap();
        assert!((24_000..=36_000).contains(&d3), "capped at 30 s ± jitter");
        assert_eq!(retry_delay(&p, 4, None), None);
        assert!(retry_delay(&p, 1, Some(Duration::from_secs(120))).unwrap() >= 120_000);
    }
}
