pub mod admin;
pub mod auth;
pub mod ingest;
pub mod oidc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let protected = admin::routes()
        .merge(auth::passkey_routes())
        .merge(oidc::identity_routes())
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_session,
        ));
    Router::new()
        .route("/api/v1/notify", post(ingest::notify))
        .route("/healthz", get(healthz))
        .nest("/api/auth", auth::routes().merge(oidc::routes()))
        .nest("/api", protected)
        .fallback_service(web(&state.config.web_dir))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// The built GUI. Any path that isn't a file gets `index.html`, so the GUI's own routes load.
fn web(dir: &std::path::Path) -> Router {
    let spa = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
    Router::new()
        .fallback_service(spa)
        .layer(axum::middleware::from_fn(cache_control))
}

/// Vite names the files in `/assets/` by their content, so they can be cached for good.
/// `index.html` must be checked on every load, or a browser keeps a stale copy that loads the
/// previous version's assets after an update.
async fn cache_control(req: Request, next: Next) -> Response {
    let asset = req.uri().path().starts_with("/assets/");
    let mut res = next.run(req).await;
    // A missing asset gets index.html from the fallback; that mustn't be cached for good.
    let html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("text/html"));
    let value = if asset && !html && res.status().is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    res
}

async fn healthz(axum::extract::State(state): axum::extract::State<AppState>) -> Response {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => "ok\n".into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, format!("database: {e}\n")).into_response(),
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    details: Vec<String>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            details: Vec::new(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }
    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("no such {what}"))
    }
    pub fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!("internal error: {e}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error; see the server log",
        )
    }
    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.message });
        if !self.details.is_empty() {
            body["details"] = json!(self.details);
        }
        (self.status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt;

    async fn cache_control(web: &axum::Router, path: &str) -> (u16, String) {
        let res = web
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let value = res.headers()[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .to_string();
        (res.status().as_u16(), value)
    }

    #[tokio::test]
    async fn caches_assets_but_not_the_page() {
        let dir = std::env::temp_dir().join(format!("switchboard-web-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
        std::fs::write(dir.join("assets/index-abc.js"), "1").unwrap();
        let web = super::web(&dir);

        let immutable = "public, max-age=31536000, immutable".to_string();
        assert_eq!(
            cache_control(&web, "/assets/index-abc.js").await,
            (200, immutable)
        );
        assert_eq!(cache_control(&web, "/").await, (200, "no-cache".into()));
        assert_eq!(
            cache_control(&web, "/notifications").await,
            (200, "no-cache".into())
        );
        // A missing asset falls back to the page, which must not be cached for good.
        assert_eq!(
            cache_control(&web, "/assets/index-old.js").await.1,
            "no-cache"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }
}
