//! `POST /api/v1/notify`: the one endpoint every sender uses.
//!
//! The handler checks the token, validates the notification, stores it and answers `202` with its
//! id. Routing and delivery happen afterwards, so a sender never waits for a slow target.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use serde_json::{Value, json};

use super::{ApiError, ApiResult};
use crate::notification::Input;
use crate::state::AppState;
use crate::util::{now_ms, sha256_hex};

pub async fn notify(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .ok_or_else(|| {
            ApiError::unauthorized("send the source's token as `Authorization: Bearer <token>`")
        })?;
    let node_id: Option<String> =
        sqlx::query_scalar("SELECT node_id FROM source_tokens WHERE token_hash = ?")
            .bind(sha256_hex(token))
            .fetch_optional(&state.db)
            .await?;
    let graph = state.graph();
    let (node_id, source) = node_id
        .and_then(|id| graph.source(&id).map(|s| (id, s.name.clone())))
        .ok_or_else(|| ApiError::unauthorized("unknown token"))?;

    let input = parse(&headers, &body)?;
    input.validate().map_err(ApiError::bad_request)?;

    let id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO notifications (id, source_id, source_name, received_at, title, body, priority, tags, url, data, status)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'received')",
    )
    .bind(&id)
    .bind(&node_id)
    .bind(&source)
    .bind(now_ms())
    .bind(input.title.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(input.body.as_deref().filter(|s| !s.trim().is_empty()))
    .bind(input.priority.unwrap_or(3))
    .bind(json!(input.tags).to_string())
    .bind(&input.url)
    .bind(Value::Object(input.data).to_string())
    .execute(&state.db)
    .await?;

    // If this fails the router has stopped; the notification is stored and is routed at the next start.
    let _ = state.route_tx.send(id.clone());
    state.emit("received", &id);
    Ok((StatusCode::ACCEPTED, Json(json!({ "id": id }))))
}

/// JSON, or for quick shell use, a plain-text body with optional `Title`, `Priority` and `Tags` headers.
fn parse(headers: &HeaderMap, body: &[u8]) -> ApiResult<Input> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("text/plain") {
        let text =
            std::str::from_utf8(body).map_err(|_| ApiError::bad_request("the body isn't UTF-8"))?;
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        return Ok(Input {
            title: header("title"),
            body: Some(text.to_string()),
            priority: header("priority")
                .map(|p| {
                    p.trim()
                        .parse()
                        .map_err(|_| ApiError::bad_request("Priority must be 1 to 5"))
                })
                .transpose()?,
            tags: header("tags")
                .map(|t| {
                    t.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            url: header("url"),
            data: Default::default(),
        });
    }
    serde_json::from_slice(body)
        .map_err(|e| ApiError::bad_request(format!("invalid notification: {e}")))
}
