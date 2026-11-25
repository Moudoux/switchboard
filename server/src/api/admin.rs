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
use crate::graph::{Graph, NodeKind};
use crate::notification::{Input, Notification};
use crate::state::AppState;
use crate::util::{now_ms, random_token, sha256_hex};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/graph", get(get_graph))
        .route("/graph", put(put_graph))
        .route("/sources/{id}/token", post(new_token))
        .route("/targets", get(list_targets))
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

/// Every target in the graph, and every target that has deliveries in the history, for the
/// history's filter. Names are the graph's, or the latest a delivery recorded.
async fn list_targets(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let graph = state.graph();
    let mut targets: Vec<Value> = graph
        .targets()
        .map(|(id, t)| json!({ "id": id, "name": t.name(), "inGraph": true }))
        .collect();
    let history: Vec<(String, String)> = sqlx::query_as(
        "SELECT target_id, target_name FROM deliveries d
         WHERE created_at = (SELECT MAX(created_at) FROM deliveries e WHERE e.target_id = d.target_id)
         GROUP BY target_id",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, name) in history {
        if graph.target(&id).is_none() {
            targets.push(json!({ "id": id, "name": name, "inGraph": false }));
        }
    }
    Ok(Json(json!(targets)))
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    /// `receivedAt` of the last row of the previous page.
    before: Option<i64>,
    source: Option<String>,
    /// Only notifications with a delivery to this target; the status counts are then its own.
    target: Option<String>,
    /// delivered, failed, pending, held, skipped, unrouted, duplicate
    status: Option<String>,
    q: Option<String>,
}

/// Delivery counts for each notification `n`. With `for_target`, only the deliveries to the
/// target bound as `?5` count (a fallback that saved one still counts as saving it).
fn summary(for_target: bool) -> String {
    let only = if for_target {
        " AND d.target_id = ?5"
    } else {
        ""
    };
    format!(
        "
    (SELECT COUNT(*) FROM deliveries d WHERE d.notification_id = n.id{only} AND d.status = 'delivered') AS delivered,
    (SELECT COUNT(*) FROM deliveries d WHERE d.notification_id = n.id{only} AND d.status IN ('pending', 'sending')) AS pending,
    (SELECT COUNT(*) FROM deliveries d WHERE d.notification_id = n.id{only} AND d.status = 'pending'
        AND d.attempts = 0 AND d.held_until IS NOT NULL) AS held,
    (SELECT COUNT(*) FROM deliveries d WHERE d.notification_id = n.id{only} AND d.status = 'skipped') AS skipped,
    (SELECT COUNT(*) FROM deliveries d WHERE d.notification_id = n.id{only} AND d.status = 'failed'
        AND NOT EXISTS (SELECT 1 FROM deliveries f WHERE f.parent_id = d.id AND f.status = 'delivered')) AS failed"
    )
}

#[derive(sqlx::FromRow)]
struct Row {
    #[sqlx(flatten)]
    n: Notification,
    status: String,
    #[sqlx(json)]
    notes: Value,
    delivered: i64,
    pending: i64,
    held: i64,
    skipped: i64,
    failed: i64,
}

impl Row {
    fn to_json(&self) -> Value {
        let mut v = serde_json::to_value(&self.n).unwrap_or_default();
        v["routing"] = json!(self.status);
        v["notes"] = self.notes.clone();
        v["deliveries"] = json!({
            "delivered": self.delivered,
            "pending": self.pending,
            "held": self.held,
            "skipped": self.skipped,
            "failed": self.failed,
        });
        v
    }
}

async fn list_notifications(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let target = q.target.as_deref().filter(|t| !t.is_empty());
    let summary = summary(target.is_some());
    let mut sql = format!("SELECT n.*, {summary} FROM notifications n WHERE n.received_at < ?1");
    if q.source.is_some() {
        sql.push_str(" AND n.source_id = ?2");
    }
    if q.q.is_some() {
        sql.push_str(" AND (n.title LIKE ?3 OR n.body LIKE ?3)");
    }
    if target.is_some() {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM deliveries t WHERE t.notification_id = n.id AND t.target_id = ?5)",
        );
    }
    let outer = match q.status.as_deref() {
        Some("delivered") => "WHERE delivered > 0 AND pending = 0 AND failed = 0",
        Some("failed") => "WHERE failed > 0",
        Some("pending") => "WHERE pending > held OR status = 'received'",
        Some("held") => "WHERE held > 0",
        Some("skipped") => "WHERE skipped > 0",
        Some("unrouted") => "WHERE status = 'unrouted'",
        Some("duplicate") => "WHERE status = 'duplicate'",
        _ => "",
    };
    let sql = format!("SELECT * FROM ({sql}) {outer} ORDER BY received_at DESC LIMIT ?4");
    // `sql` is built from constant strings only; the user's values are bound.
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(q.before.unwrap_or(i64::MAX))
        .bind(&q.source)
        .bind(q.q.as_ref().map(|s| format!("%{s}%")))
        .bind(limit)
        .bind(target)
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
    held_until: Option<i64>,
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
        "SELECT n.*, {} FROM notifications n WHERE n.id = ?1",
        summary(false)
    )))
    .bind(&id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("notification"))?;
    let mut deliveries: Vec<DeliveryRow> = sqlx::query_as(
        "SELECT id, target_id, target_name, parent_id, status, attempts, next_attempt_at, held_until, last_error, created_at, finished_at
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

/// Tries a finished or skipped delivery's target again now, as a new delivery with fresh retries.
async fn retry_delivery(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT notification_id, target_id FROM deliveries WHERE id = ? AND status IN ('failed', 'delivered', 'skipped')")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let (notification_id, target_id) = row.ok_or_else(|| {
        ApiError::bad_request("only a finished or skipped delivery can be retried")
    })?;
    if state.graph().target(&target_id).is_none() {
        return Err(ApiError::bad_request(
            "that target is no longer in the graph",
        ));
    }
    let mut tx = state.db.begin().await?;
    engine::create_deliveries(&state, &mut tx, &notification_id, &[target_id], None, None).await?;
    tx.commit().await?;
    state.wake.notify_one();
    state.emit("delivery", &notification_id);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TestRequest {
    /// The node type: `http` (the default) or `apprise`.
    #[serde(default = "default_test_type", rename = "type")]
    kind: String,
    /// The node's data, as in the graph.
    target: Value,
    notification: Option<Input>,
}

fn default_test_type() -> String {
    "http".into()
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
    let node: NodeKind = serde_json::from_value(json!({ "type": req.kind, "data": req.target }))
        .map_err(|e| ApiError::bad_request(format!("the target doesn't parse: {e}")))?;
    let target = node
        .as_target()
        .ok_or_else(|| ApiError::bad_request("only targets can be tested"))?;
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
    let outcome = send::send(&state.http, target, &n.template_value()).await;
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

#[cfg(test)]
mod tests {
    //! Routing, dedupe, quiet hours and the history's filters, through the real router and
    //! handlers against a scratch database. The delivery worker isn't running, so nothing is sent.

    use super::*;
    use crate::config::Config;

    async fn state(graph: Value) -> (AppState, std::path::PathBuf) {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let path =
            std::env::temp_dir().join(format!("switchboard-test-{}.db", uuid::Uuid::now_v7()));
        let db = crate::db::connect(&path).await.unwrap();
        let config = Config {
            listen: String::new(),
            db_path: path.clone(),
            web_dir: Default::default(),
            retention_days: 30,
            concurrency: 1,
            cookie_secure: false,
            origin: "http://localhost:8080".into(),
        };
        let graph: Graph = serde_json::from_value(graph).unwrap();
        assert!(graph.validate().is_empty(), "{:?}", graph.validate());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        (AppState::new(config, db, graph, tx).unwrap(), path)
    }

    async fn notify(state: &AppState, title: &str, priority: i64) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO notifications (id, source_id, source_name, received_at, title, body, priority, tags, url, data, status)
             VALUES (?, 'src', 'Backup', ?, ?, NULL, ?, '[]', NULL, '{}', 'received')",
        )
        .bind(&id)
        .bind(now_ms())
        .bind(title)
        .bind(priority)
        .execute(&state.db)
        .await
        .unwrap();
        engine::router::route(state, &id, false).await.unwrap();
        id
    }

    async fn list(state: &AppState, target: Option<&str>, status: Option<&str>) -> Vec<Value> {
        let q = ListQuery {
            limit: None,
            before: None,
            source: None,
            target: target.map(str::to_string),
            status: status.map(str::to_string),
            q: None,
        };
        let Json(v) = list_notifications(State(state.clone()), Query(q))
            .await
            .unwrap_or_else(|_| panic!("listing failed"));
        v.as_array().unwrap().clone()
    }

    fn titles(rows: &[Value]) -> Vec<&str> {
        rows.iter().map(|r| r["title"].as_str().unwrap()).collect()
    }

    #[tokio::test]
    async fn dedupe_quiet_hours_and_target_filter() {
        // Quiet all day, every day: `night` holds (priority 5 goes through), `dropper` drops.
        let all_day = |action: &str, min: Value| {
            json!({"enabled": true, "start": "00:00", "end": "00:00", "days": [0,1,2,3,4,5,6],
                   "timezone": "UTC", "action": action, "minPriority": min})
        };
        let (state, path) = state(json!({
            "nodes": [
                {"id": "src", "type": "source", "data": {"name": "Backup"}},
                {"id": "dd", "type": "dedup", "data": {"name": "Once", "key": "{{ n.title }}", "windowSecs": 600}},
                {"id": "phone", "type": "apprise", "data": {"name": "Phone", "url": "http://apprise:8000", "urls": ["ntfy://t"]}},
                {"id": "night", "type": "http", "data": {"name": "Night", "url": "http://x", "quietHours": all_day("hold", json!(5))}},
                {"id": "dropper", "type": "http", "data": {"name": "Dropper", "url": "http://y", "quietHours": all_day("drop", Value::Null)}}
            ],
            "edges": [
                {"id": "e1", "source": "src", "target": "dd"},
                {"id": "e2", "source": "dd", "target": "phone", "sourceHandle": "new"},
                {"id": "e3", "source": "src", "target": "night"},
                {"id": "e4", "source": "src", "target": "dropper"}
            ]
        }))
        .await;

        let first = notify(&state, "Disk full", 3).await;
        let repeat = notify(&state, "Disk full", 3).await;
        notify(&state, "Fire", 5).await;

        let all = list(&state, None, None).await;
        assert_eq!(titles(&all), ["Fire", "Disk full", "Disk full"]);
        // The repeat still reaches the targets outside the dedupe node, so it's routed, with a note.
        assert_eq!(all[1]["id"], json!(repeat));
        assert_eq!(all[1]["routing"], "routed");
        assert_eq!(all[1]["notes"][0]["notificationId"], json!(first));
        assert_eq!(all[2]["deliveries"]["held"], 1);
        assert_eq!(all[2]["deliveries"]["skipped"], 1);
        // Priority 5 isn't held, but nothing gets through the drop.
        assert_eq!(all[0]["deliveries"]["held"], 0);
        assert_eq!(all[0]["deliveries"]["pending"], 2);
        assert_eq!(all[0]["deliveries"]["skipped"], 1);

        // Filtering by target, with that target's own counts.
        let phone = list(&state, Some("phone"), None).await;
        assert_eq!(phone.len(), 2, "the repeat never reached the phone");
        assert_eq!(phone[1]["deliveries"]["held"], 0);
        assert_eq!(phone[1]["deliveries"]["pending"], 1);
        assert_eq!(
            titles(&list(&state, Some("night"), Some("held")).await),
            ["Disk full", "Disk full"]
        );
        assert_eq!(
            titles(&list(&state, Some("night"), Some("pending")).await),
            ["Fire"]
        );
        assert_eq!(
            list(&state, Some("dropper"), Some("skipped")).await.len(),
            3
        );
        assert!(
            list(&state, Some("phone"), Some("skipped"))
                .await
                .is_empty()
        );

        let held_until: Option<i64> = sqlx::query_scalar(
            "SELECT held_until FROM deliveries WHERE notification_id = ? AND target_id = 'night'",
        )
        .bind(&first)
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert!(held_until.unwrap() > now_ms());

        // A repeat with only the dedupe node in its way is a duplicate.
        let mut graph = (*state.graph()).clone();
        graph.edges.retain(|e| e.id == "e1" || e.id == "e2");
        state.set_graph(graph);
        let dup = notify(&state, "Disk full", 3).await;
        assert_eq!(
            titles(&list(&state, None, Some("duplicate")).await),
            ["Disk full"]
        );

        // Sending it again by hand ignores dedupe.
        let Json(r) = redeliver(State(state.clone()), Path(dup.clone()))
            .await
            .unwrap_or_else(|_| panic!("redeliver failed"));
        assert_eq!(r["deliveries"], 1);
        assert!(list(&state, None, Some("duplicate")).await.is_empty());

        // The target list has what's in the graph and what's only in the history.
        let Json(targets) = list_targets(State(state.clone()))
            .await
            .unwrap_or_else(|_| panic!("targets failed"));
        let names: Vec<(&str, bool)> = targets
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["name"].as_str().unwrap(), t["inGraph"].as_bool().unwrap()))
            .collect();
        assert!(names.contains(&("Phone", true)));
        assert!(names.contains(&("Night", true)));

        let mut graph = (*state.graph()).clone();
        graph.nodes.retain(|n| n.id != "night");
        state.set_graph(graph);
        let Json(targets) = list_targets(State(state.clone()))
            .await
            .unwrap_or_else(|_| panic!("targets failed"));
        assert!(
            targets
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "Night" && t["inGraph"] == false)
        );

        state.db.close().await;
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
