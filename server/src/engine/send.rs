use std::time::{Duration, Instant};

use serde::Serialize;

use serde_json::json;

use crate::graph::{AppriseFormat, AppriseTarget, Header, HttpTarget, Target};
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
    let body = template::render(&target.body, n).map_err(|e| format!("body template: {e}"))?;
    Ok(RenderedRequest {
        method: target.method.to_uppercase(),
        url,
        headers: render_headers(&target.headers, n)?,
        body,
    })
}

fn render_headers(
    headers: &[Header],
    n: &serde_json::Value,
) -> Result<Vec<(String, String)>, String> {
    headers
        .iter()
        .map(|h| {
            let value =
                template::render(&h.value, n).map_err(|e| format!("header {}: {e}", h.name))?;
            Ok((h.name.clone(), value.trim().to_string()))
        })
        .collect()
}

/// The request to an Apprise API server: `/notify/` with the URLs, or `/notify/<key>` with a tag.
pub fn render_apprise(
    target: &AppriseTarget,
    n: &serde_json::Value,
) -> Result<RenderedRequest, String> {
    let title = template::render(&target.title, n).map_err(|e| format!("title template: {e}"))?;
    let mut body = template::render(&target.body, n).map_err(|e| format!("body template: {e}"))?;
    if body.trim().is_empty() {
        // Apprise needs a body; the title is better than nothing.
        body = title.clone();
    }
    let priority = n["priority"].as_i64().unwrap_or(3);
    let kind = match priority {
        5.. => "failure",
        4 => "warning",
        _ => "info",
    };
    let format = match target.format {
        AppriseFormat::Text => "text",
        AppriseFormat::Markdown => "markdown",
        AppriseFormat::Html => "html",
    };
    let mut payload = json!({
        "title": title.trim(),
        "body": body,
        "type": kind,
        "format": format,
    });
    let base = target.url.trim().trim_end_matches('/');
    let key = target.key.trim();
    let url = if key.is_empty() {
        let urls: Vec<&str> = target
            .urls
            .iter()
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
            .collect();
        payload["urls"] = json!(urls.join(", "));
        format!("{base}/notify/")
    } else {
        if !target.tag.trim().is_empty() {
            payload["tag"] = json!(target.tag.trim());
        }
        format!("{base}/notify/{key}")
    };
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    headers.extend(render_headers(&target.headers, n)?);
    Ok(RenderedRequest {
        method: "POST".into(),
        url,
        headers,
        body: payload.to_string(),
    })
}

pub async fn send(client: &reqwest::Client, target: Target<'_>, n: &serde_json::Value) -> Outcome {
    let started = Instant::now();
    let rendered = match target {
        Target::Http(t) => render(t, n),
        Target::Apprise(t) => render_apprise(t, n),
    };
    let mut outcome = match rendered {
        Ok(request) => execute(client, request, target.timeout_secs(), started).await,
        Err(e) => failed(started, e, false, None),
    };
    if let Target::Apprise(_) = target
        && outcome.http_status == Some(424)
    {
        // Apprise answers 424 when a service it notified failed, which is usually worth retrying.
        outcome.retryable = true;
        outcome.error = Some(
            "HTTP 424: Apprise couldn't notify at least one service, or no URL matched the tag"
                .into(),
        );
    }
    outcome
}

fn failed(
    started: Instant,
    error: String,
    retryable: bool,
    request: Option<RenderedRequest>,
) -> Outcome {
    Outcome {
        ok: false,
        retryable,
        http_status: None,
        error: Some(error),
        response: None,
        duration_ms: started.elapsed().as_millis() as i64,
        retry_after: None,
        request,
    }
}

async fn execute(
    client: &reqwest::Client,
    request: RenderedRequest,
    timeout_secs: u64,
    started: Instant,
) -> Outcome {
    let method = match reqwest::Method::from_bytes(request.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return failed(
                started,
                format!("bad method {}", request.method),
                false,
                Some(request),
            );
        }
    };
    let mut builder = client
        .request(method, &request.url)
        .timeout(Duration::from_secs(timeout_secs));
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
            return failed(started, describe(&e), retryable, Some(request));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_apprise_requests() {
        let mut t: AppriseTarget = serde_json::from_value(json!({
            "name": "Phone", "url": "http://apprise:8000/", "urls": ["ntfy://a", " ", "tgram://b/c"], "format": "markdown"
        }))
        .unwrap();
        let n =
            json!({"title": null, "text": "disk full", "priority": 5, "source": {"name": "NAS"}});
        let r = render_apprise(&t, &n).unwrap();
        assert_eq!(r.url, "http://apprise:8000/notify/");
        let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(
            body,
            json!({"urls": "ntfy://a, tgram://b/c", "title": "NAS", "body": "disk full", "type": "failure", "format": "markdown"})
        );

        t.urls.clear();
        t.key = "home".into();
        t.tag = "ops, admins".into();
        let r = render_apprise(&t, &json!({"title": "Hi", "text": "x", "priority": 2})).unwrap();
        assert_eq!(r.url, "http://apprise:8000/notify/home");
        let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(body["tag"], "ops, admins");
        assert_eq!(body["type"], "info");
        assert!(body.get("urls").is_none());
    }
}
