//! The GUI's API. Every route here needs a session.

use std::collections::HashSet;
use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use super::{ApiError, ApiResult};
use crate::engine::{self, send};
use crate::graph::{Graph, HttpTarget, NodeKind};
use crate::notification::{Input, Notification};
use crate::state::AppState;
use crate::util::{now_ms, random_token, sha256_hex};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/graph", get(get_graph))
        .route("/graph", put(put_graph))
        .route("/sources/{id}/token", post(new_token))
        .route("/notifications", get(list_notifications))
        .route("/notifications/{id}", get(get_notification))
        .route("/notifications/{id}/redeliver", post(redeliver))
        .route("/deliveries/{id}/retry", post(retry_delivery))
        .route("/test", post(test_target))
        .route("/events", get(events))
}

async fn get_graph(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let (version, graph) = crate::db::load_graph(&state.db)
        .await
        .map_err(ApiError::internal)?;
    let tokens: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT node_id, prefix, created_at FROM source_tokens")
            .fetch_all(&state.db)
            .await?;
    let tokens: serde_json::Map<String, Value> = tokens
        .into_iter()
        .map(|(id, prefix, created)| (id, json!({ "prefix": prefix, "createdAt": created })))
        .collect();
    Ok(Json(
        json!({ "version": version, "graph": graph, "tokens": tokens }),
    ))
}

#[derive(Deserialize)]
struct PutGraph {
    /// The version the GUI started from, so two tabs don't overwrite each other's changes.
    version: i64,
    graph: Value,
}

async fn put_graph(
    State(state): State<AppState>,
    Json(req): Json<PutGraph>,
) -> ApiResult<Json<Value>> {
    let graph: Graph = serde_json::from_value(req.graph.clone())
        .map_err(|e| ApiError::bad_request(format!("the graph doesn't parse: {e}")))?;
    let errors = graph.validate();
    if !errors.is_empty() {
        return Err(ApiError::bad_request("the graph has problems").with_details(errors));
    }
    let mut tx = state.db.begin().await?;
    let current: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM graph_versions")
        .fetch_one(&mut *tx)
        .await?;
    if current != req.version {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "the graph was changed elsewhere since you opened it; reload to see it",
        ));
    }
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO graph_versions (created_at, graph) VALUES (?, ?) RETURNING id",
    )
    .bind(now_ms())
    .bind(req.graph.to_string())
    .fetch_one(&mut *tx)
    .await?;
    // A source that was removed loses its token, so a re-added node with the same id doesn't inherit it.
    let sources: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Source(_)))
        .map(|n| n.id.as_str())
        .collect();
    let token_nodes: Vec<String> = sqlx::query_scalar("SELECT node_id FROM source_tokens")
        .fetch_all(&mut *tx)
        .await?;
    for id in token_nodes
        .iter()
        .filter(|id| !sources.contains(id.as_str()))
    {
        sqlx::query("DELETE FROM source_tokens WHERE node_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    state.set_graph(graph);
    tracing::info!(version, "graph saved");
    Ok(Json(json!({ "version": version })))
}

/// Makes a new token for a source, replacing its old one. The token is shown this once.
async fn new_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    if state.graph().source(&id).is_none() {
        return Err(ApiError::bad_request(
            "save the graph with this source first",
        ));
    }
    let token = random_token("sb_");
    let prefix: String = token.chars().take(10).collect();
    sqlx::query(
        "INSERT INTO source_tokens (node_id, token_hash, prefix, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (node_id) DO UPDATE SET token_hash = ?2, prefix = ?3, created_at = ?4",
    )
    .bind(&id)
    .bind(sha256_hex(&token))
    .bind(&prefix)
    .bind(now_ms())
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "token": token, "prefix": prefix })))
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    /// `receivedAt` of the last row of the previous page.
    before: Option<i64>,
    source: Option<String>,
    /// delivered, failed, pending, unrouted
    status: Option<String>,
    q: Option<String>,
}

const SUMMARY: &str = "
    (SELECT COUNT(*) FROM deliveries d WHERE d.notification_id = n.id AND d.status = 'delivered') AS delivered,
    (SELECT COUNT(*) FROM deliveries d WHERE d.notification_id = n.id AND d.status IN ('pending', 'sending')) AS pending,
    (SELECT COUNT(*) FROM deliveries d WHERE d.notification_id = n.id AND d.status = 'failed'
        AND NOT EXISTS (SELECT 1 FROM deliveries f WHERE f.parent_id = d.id AND f.status = 'delivered')) AS failed";

#[derive(sqlx::FromRow)]
struct Row {
    #[sqlx(flatten)]
    n: Notification,
    status: String,
    delivered: i64,
    pending: i64,
    failed: i64,
}

impl Row {
    fn to_json(&self) -> Value {
        let mut v = serde_json::to_value(&self.n).unwrap_or_default();
        v["routing"] = json!(self.status);
        v["deliveries"] =
            json!({ "delivered": self.delivered, "pending": self.pending, "failed": self.failed });
        v
    }
}

async fn list_notifications(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let mut sql = format!("SELECT n.*, {SUMMARY} FROM notifications n WHERE n.received_at < ?1");
    if q.source.is_some() {
        sql.push_str(" AND n.source_id = ?2");
    }
    if q.q.is_some() {
        sql.push_str(" AND (n.title LIKE ?3 OR n.body LIKE ?3)");
    }
    let outer = match q.status.as_deref() {
        Some("delivered") => "WHERE delivered > 0 AND pending = 0 AND failed = 0",
        Some("failed") => "WHERE failed > 0",
        Some("pending") => "WHERE pending > 0 OR status = 'received'",
        Some("unrouted") => "WHERE status = 'unrouted'",
        _ => "",
    };
    let sql = format!("SELECT * FROM ({sql}) {outer} ORDER BY received_at DESC LIMIT ?4");
    // `sql` is built from constant strings only; the user's values are bound.
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(q.before.unwrap_or(i64::MAX))
        .bind(&q.source)
        .bind(q.q.as_ref().map(|s| format!("%{s}%")))
        .bind(limit)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!(
        rows.iter().map(Row::to_json).collect::<Vec<_>>()
    )))
}

#[derive(sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryRow {
    id: String,
    target_id: String,
    target_name: String,
    parent_id: Option<String>,
    status: String,
    attempts: i64,
    next_attempt_at: i64,
    last_error: Option<String>,
    created_at: i64,
    finished_at: Option<i64>,
    #[sqlx(skip)]
    tries: Vec<AttemptRow>,
}

#[derive(sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AttemptRow {
    #[serde(skip)]
    delivery_id: String,
    started_at: i64,
    duration_ms: i64,
    ok: bool,
    http_status: Option<i64>,
    error: Option<String>,
    response: Option<String>,
}

async fn get_notification(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT n.*, {SUMMARY} FROM notifications n WHERE n.id = ?"
    )))
    .bind(&id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("notification"))?;
    let mut deliveries: Vec<DeliveryRow> = sqlx::query_as(
        "SELECT id, target_id, target_name, parent_id, status, attempts, next_attempt_at, last_error, created_at, finished_at
         FROM deliveries WHERE notification_id = ? ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    let attempts: Vec<AttemptRow> = sqlx::query_as(
        "SELECT a.delivery_id, a.started_at, a.duration_ms, a.ok, a.http_status, a.error, a.response
         FROM attempts a JOIN deliveries d ON d.id = a.delivery_id WHERE d.notification_id = ? ORDER BY a.id",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    for attempt in attempts {
        if let Some(d) = deliveries.iter_mut().find(|d| d.id == attempt.delivery_id) {
            d.tries.push(attempt);
        }
    }
    let mut v = row.to_json();
    v["deliveryList"] = json!(deliveries);
    Ok(Json(v))
}

/// Routes a notification again through the current graph, as if it had just arrived.
async fn redeliver(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let created = engine::router::route(&state, &id, true).await?;
    Ok(Json(json!({ "deliveries": created })))
}

/// Tries a finished delivery's target again, as a new delivery with fresh retries.
async fn retry_delivery(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT notification_id, target_id FROM deliveries WHERE id = ? AND status IN ('failed', 'delivered')")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let (notification_id, target_id) =
        row.ok_or_else(|| ApiError::bad_request("only a finished delivery can be retried"))?;
    if state.graph().target(&target_id).is_none() {
        return Err(ApiError::bad_request(
            "that target is no longer in the graph",
        ));
    }
    let mut tx = state.db.begin().await?;
    engine::create_deliveries(&state, &mut tx, &notification_id, &[target_id], None).await?;
    tx.commit().await?;
    state.wake.notify_one();
    state.emit("delivery", &notification_id);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TestRequest {
    target: HttpTarget,
    notification: Option<Input>,
}

/// Sends one request to a target as configured in the GUI (saved or not), and shows what happened.
async fn test_target(
    State(state): State<AppState>,
    Json(req): Json<TestRequest>,
) -> ApiResult<Json<Value>> {
    let input = req.notification.unwrap_or(Input {
        title: Some("Test notification".into()),
        body: Some("Sent from Switchboard's GUI.".into()),
        priority: Some(3),
        tags: vec!["test".into()],
        url: None,
        data: Default::default(),
    });
    input.validate().map_err(ApiError::bad_request)?;
    let n = Notification {
        id: "test".into(),
        source_id: "test".into(),
        source_name: "Test".into(),
        received_at: now_ms(),
        title: input.title,
        body: input.body,
        priority: input.priority.unwrap_or(3),
        tags: input.tags,
        url: input.url,
        data: Value::Object(input.data),
    };
    let outcome = send::send(&state.http, &req.target, &n.template_value()).await;
    Ok(Json(
        serde_json::to_value(outcome).map_err(ApiError::internal)?,
    ))
}

/// Server-sent events telling the GUI what to refresh.
async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(state.events.subscribe()).filter_map(|e| {
        // A lagging client misses some events; it refreshes on the next one anyway.
        let e = e.ok()?;
        Some(Ok(SseEvent::default().json_data(e).unwrap_or_default()))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
