use std::time::{Duration, Instant};

use serde::Serialize;

use crate::graph::HttpTarget;
use crate::template;

/// What one attempt sent and got back.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub ok: bool,
    /// Worth trying again: network errors, timeouts, 408, 425, 429 and 5xx.
    #[serde(skip)]
    pub retryable: bool,
    pub http_status: Option<u16>,
    pub error: Option<String>,
    pub response: Option<String>,
    pub duration_ms: i64,
    /// From a Retry-After header in seconds.
    #[serde(skip)]
    pub retry_after: Option<Duration>,
    pub request: Option<RenderedRequest>,
}

#[derive(Debug, Serialize)]
pub struct RenderedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

const MAX_RESPONSE: usize = 2_000;

pub fn render(target: &HttpTarget, n: &serde_json::Value) -> Result<RenderedRequest, String> {
    let url = template::render(&target.url, n)
        .map_err(|e| format!("URL template: {e}"))?
        .trim()
        .to_string();
    let mut headers = Vec::new();
    for h in &target.headers {
        let value = template::render(&h.value, n).map_err(|e| format!("header {}: {e}", h.name))?;
        headers.push((h.name.clone(), value.trim().to_string()));
    }
    let body = template::render(&target.body, n).map_err(|e| format!("body template: {e}"))?;
    Ok(RenderedRequest {
        method: target.method.to_uppercase(),
        url,
        headers,
        body,
    })
}

pub async fn send(client: &reqwest::Client, target: &HttpTarget, n: &serde_json::Value) -> Outcome {
    let started = Instant::now();
    let failed = |error: String, retryable: bool, request| Outcome {
        ok: false,
        retryable,
        http_status: None,
        error: Some(error),
        response: None,
        duration_ms: started.elapsed().as_millis() as i64,
        retry_after: None,
        request,
    };
    let request = match render(target, n) {
        Ok(r) => r,
        Err(e) => return failed(e, false, None),
    };
    let method = match reqwest::Method::from_bytes(request.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return failed(
                format!("bad method {}", request.method),
                false,
                Some(request),
            );
        }
    };
    let mut builder = client
        .request(method, &request.url)
        .timeout(Duration::from_secs(target.timeout_secs));
    let mut has_content_type = false;
    for (name, value) in &request.headers {
        has_content_type |= name.eq_ignore_ascii_case("content-type");
        builder = builder.header(name, value);
    }
    if !request.body.is_empty() {
        if !has_content_type {
            let json = serde_json::from_str::<serde_json::Value>(&request.body).is_ok();
            builder = builder.header(
                "content-type",
                if json {
                    "application/json"
                } else {
                    "text/plain; charset=utf-8"
                },
            );
        }
        builder = builder.body(request.body.clone());
    }
    let response = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            let retryable = !e.is_builder();
            return failed(describe(&e), retryable, Some(request));
        }
    };
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|s| Duration::from_secs_f64(s.clamp(0.0, 86_400.0)));
    let text = response.text().await.unwrap_or_default();
    let mut text: String = text.chars().take(MAX_RESPONSE).collect();
    if text.len() == MAX_RESPONSE {
        text.push('…');
    }
    let ok = status.is_success();
    let code = status.as_u16();
    Outcome {
        ok,
        retryable: !ok && (status.is_server_error() || matches!(code, 408 | 425 | 429)),
        http_status: Some(code),
        error: (!ok).then(|| format!("HTTP {status}")),
        response: (!text.is_empty()).then_some(text),
        duration_ms: started.elapsed().as_millis() as i64,
        retry_after,
        request: Some(request),
    }
}

/// A short reason, such as "can't connect: Connection refused (os error 111)". reqwest's own text
/// repeats the URL, which the GUI already shows; the innermost cause is the useful part.
fn describe(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "timed out".into();
    }
    let mut cause = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        cause = s.to_string();
        source = s.source();
    }
    if e.is_connect() {
        format!("can't connect: {cause}")
    } else if e.is_redirect() {
        format!("too many redirects: {cause}")
    } else {
        cause
    }
}
