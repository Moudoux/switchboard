//! The routing graph: what the GUI draws with React Flow, and what the engine follows.
//!
//! The graph is stored exactly as the GUI sends it (positions and all), and parsed into these
//! types for validation and routing. Fields the engine doesn't know, such as `position`, are ignored.
//!
//! - `source` nodes are where notifications come in. Each has its own token.
//! - `filter` nodes test a notification. Edges leave by the `match` handle (or no handle) when it
//!   passes, and by the `else` handle when it doesn't.
//! - `dedup` nodes let a notification through `new` the first time its key is seen within a
//!   window, and through `duplicate` after that. What they've seen is kept in SQLite.
//! - `group` nodes bundle targets: everything that reaches one goes on to every target (or
//!   group) it connects to. A group can also be a target's fallback.
//! - `http` and `apprise` nodes are targets. Edges leaving by the `fallback` handle go to the
//!   targets (or groups) tried once this target has given up.

use std::collections::{HashMap, HashSet, VecDeque};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::notification::Notification;
use crate::quiet_hours::QuietHours;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Graph {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(flatten)]
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "lowercase")]
pub enum NodeKind {
    Source(SourceNode),
    Filter(FilterNode),
    Dedup(DedupNode),
    Group(GroupNode),
    Http(HttpTarget),
    Apprise(AppriseTarget),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceNode {
    pub name: String,
}

/// Passes everything on to the targets and groups it connects to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupNode {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterNode {
    pub name: String,
    #[serde(default)]
    pub mode: MatchMode,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DedupNode {
    pub name: String,
    /// A template; notifications that render the same key are duplicates of each other.
    #[serde(default = "default_dedup_key")]
    pub key: String,
    /// A notification is a duplicate when one with the same key passed less than this long ago.
    #[serde(default = "default_dedup_window")]
    pub window_secs: u64,
}

fn default_dedup_key() -> String {
    "{{ n.source.id }}|{{ n.title }}|{{ n.body }}".into()
}

fn default_dedup_window() -> u64 {
    600
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    #[default]
    All,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// `title`, `body`, `priority`, `tags`, `source`, `url`, or `data.<path>` (dots between keys).
    pub field: String,
    pub op: Op,
    #[serde(default)]
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Ne,
    Contains,
    NotContains,
    Matches,
    Gt,
    Gte,
    Lt,
    Lte,
    Exists,
    NotExists,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpTarget {
    pub name: String,
    #[serde(default = "default_method")]
    pub method: String,
    /// A template, like the header values and the body.
    pub url: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    /// The request body template. Empty sends no body.
    #[serde(default)]
    pub body: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default)]
    pub quiet_hours: QuietHours,
}

/// Sends through an [Apprise API](https://github.com/caronc/apprise-api) server, which reaches
/// the services Apprise supports (Telegram, ntfy, email, Pushover, Matrix, ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppriseTarget {
    pub name: String,
    /// The Apprise API server, such as `http://apprise:8000`.
    pub url: String,
    /// Apprise URLs to notify (stateless), such as `tgram://bot/chat`. Used when `key` is empty.
    #[serde(default)]
    pub urls: Vec<String>,
    /// A configuration saved on the server (stateful), notified as `/notify/<key>`.
    #[serde(default)]
    pub key: String,
    /// With a key: which of its URLs to notify. A comma is OR, a space is AND; `all` is every one.
    #[serde(default)]
    pub tag: String,
    #[serde(default = "default_apprise_title")]
    pub title: String,
    #[serde(default = "default_apprise_body")]
    pub body: String,
    #[serde(default)]
    pub format: AppriseFormat,
    /// Extra request headers, such as Authorization for a server behind a proxy. Values are templates.
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default)]
    pub quiet_hours: QuietHours,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppriseFormat {
    #[default]
    Text,
    Markdown,
    Html,
}

fn default_apprise_title() -> String {
    "{{ n.title or n.source.name }}".into()
}

fn default_apprise_body() -> String {
    "{{ n.text }}".into()
}

/// A target node of either kind.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    Http(&'a HttpTarget),
    Apprise(&'a AppriseTarget),
}

impl<'a> Target<'a> {
    pub fn name(self) -> &'a str {
        match self {
            Target::Http(t) => &t.name,
            Target::Apprise(t) => &t.name,
        }
    }

    pub fn retry(self) -> &'a RetryPolicy {
        match self {
            Target::Http(t) => &t.retry,
            Target::Apprise(t) => &t.retry,
        }
    }

    pub fn quiet_hours(self) -> &'a QuietHours {
        match self {
            Target::Http(t) => &t.quiet_hours,
            Target::Apprise(t) => &t.quiet_hours,
        }
    }

    pub fn timeout_secs(self) -> u64 {
        match self {
            Target::Http(t) => t.timeout_secs,
            Target::Apprise(t) => t.timeout_secs,
        }
    }
}

/// Where a notification goes, and what the dedupe nodes on its way decided.
#[derive(Debug, Default, PartialEq)]
pub struct Routing {
    /// Targets, in graph order, each once.
    pub targets: Vec<String>,
    /// Dedupe nodes it passed as new; their keys are recorded.
    pub passed: Vec<String>,
    /// Dedupe nodes that found it a duplicate.
    pub duplicates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RetryPolicy {
    /// Attempts in total, the first one included. 1 means no retries.
    pub max_attempts: u32,
    pub initial_backoff_secs: u64,
    pub max_backoff_secs: u64,
    /// Stop retrying once this long has passed since the delivery was created.
    pub give_up_after_secs: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            initial_backoff_secs: 5,
            max_backoff_secs: 600,
            give_up_after_secs: 6 * 3600,
        }
    }
}

fn default_method() -> String {
    "POST".into()
}

fn default_timeout() -> u64 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    #[serde(default)]
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub source_handle: Option<String>,
}

pub const HANDLE_MATCH: &str = "match";
pub const HANDLE_ELSE: &str = "else";
pub const HANDLE_FALLBACK: &str = "fallback";
pub const HANDLE_NEW: &str = "new";
pub const HANDLE_DUPLICATE: &str = "duplicate";

impl Graph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn source(&self, id: &str) -> Option<&SourceNode> {
        match self.node(id).map(|n| &n.kind) {
            Some(NodeKind::Source(s)) => Some(s),
            _ => None,
        }
    }

    pub fn target(&self, id: &str) -> Option<Target<'_>> {
        self.node(id).and_then(|n| n.kind.as_target())
    }

    pub fn targets(&self) -> impl Iterator<Item = (&str, Target<'_>)> {
        self.nodes
            .iter()
            .filter_map(|n| Some((n.id.as_str(), n.kind.as_target()?)))
    }

    pub fn dedup_nodes(&self) -> impl Iterator<Item = (&str, &DedupNode)> {
        self.nodes.iter().filter_map(|n| match &n.kind {
            NodeKind::Dedup(d) => Some((n.id.as_str(), d)),
            _ => None,
        })
    }

    fn edges_from<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Edge> + 'a {
        self.edges.iter().filter(move |e| e.source == id)
    }

    /// Where a notification from `source_id` goes. `is_duplicate` answers for a dedupe node, by id.
    pub fn route(
        &self,
        source_id: &str,
        n: &Notification,
        is_duplicate: impl Fn(&str) -> bool,
    ) -> Routing {
        let mut routing = Routing::default();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([source_id.to_string()]);
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(node) = self.node(&id) else { continue };
            // The output to follow, and the one an edge without a handle counts as.
            let follow: Option<(&str, &str)> = match &node.kind {
                NodeKind::Source(_) | NodeKind::Group(_) => None,
                NodeKind::Filter(f) => Some((
                    if f.matches(n) {
                        HANDLE_MATCH
                    } else {
                        HANDLE_ELSE
                    },
                    HANDLE_MATCH,
                )),
                NodeKind::Dedup(_) => Some(if is_duplicate(&id) {
                    routing.duplicates.push(id.clone());
                    (HANDLE_DUPLICATE, HANDLE_NEW)
                } else {
                    routing.passed.push(id.clone());
                    (HANDLE_NEW, HANDLE_NEW)
                }),
                NodeKind::Http(_) | NodeKind::Apprise(_) => {
                    routing.targets.push(id.clone());
                    continue; // only fallback edges leave a target; the engine follows those
                }
            };
            for edge in self.edges_from(&id) {
                let follows = match follow {
                    None => true,
                    Some((handle, default)) => {
                        edge.source_handle.as_deref().unwrap_or(default) == handle
                    }
                };
                if follows {
                    queue.push_back(edge.target.clone());
                }
            }
        }
        routing
    }

    /// The targets to try once `target_id` has given up, with groups expanded, each once.
    pub fn fallbacks(&self, target_id: &str) -> Vec<String> {
        let mut targets = Vec::new();
        let mut seen = HashSet::from([target_id.to_string()]);
        let mut queue: VecDeque<String> = self
            .edges_from(target_id)
            .filter(|e| e.source_handle.as_deref() == Some(HANDLE_FALLBACK))
            .map(|e| e.target.clone())
            .collect();
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }
            match self.node(&id).map(|n| &n.kind) {
                Some(NodeKind::Group(_)) => {
                    queue.extend(self.edges_from(&id).map(|e| e.target.clone()));
                }
                Some(k) if k.as_target().is_some() => targets.push(id),
                _ => {}
            }
        }
        targets
    }

    /// Every problem with the graph, as messages for the GUI. Empty means it can be saved.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let mut ids = HashMap::new();
        for node in &self.nodes {
            if node.id.trim().is_empty() {
                errors.push("a node has no id".into());
            }
            if ids.insert(node.id.as_str(), &node.kind).is_some() {
                errors.push(format!("node id {} is used twice", node.id));
            }
            let name = node.kind.name();
            if name.trim().is_empty() {
                errors.push(format!(
                    "{} node {} needs a name",
                    node.kind.label(),
                    node.id
                ));
            }
            match &node.kind {
                NodeKind::Source(_) | NodeKind::Group(_) => {}
                NodeKind::Filter(f) => {
                    for c in &f.conditions {
                        if let Err(e) = c.check() {
                            errors.push(format!("filter \"{name}\": {e}"));
                        }
                    }
                }
                NodeKind::Dedup(d) => {
                    if d.window_secs == 0 {
                        errors.push(format!(
                            "dedupe \"{name}\": the window must be at least 1 second"
                        ));
                    }
                    if d.key.trim().is_empty() {
                        errors.push(format!("dedupe \"{name}\" needs a key"));
                    }
                    if let Err(e) = crate::template::check(&d.key) {
                        errors.push(format!("dedupe \"{name}\": key template: {e}"));
                    }
                }
                NodeKind::Apprise(t) => {
                    check_target_common(&mut errors, name, Target::Apprise(t));
                    match reqwest::Url::parse(t.url.trim()) {
                        Ok(u) if matches!(u.scheme(), "http" | "https") => {}
                        _ => errors.push(format!(
                            "target \"{name}\" needs the Apprise API server's http(s) URL"
                        )),
                    }
                    let key = t.key.trim();
                    let urls = t.urls.iter().filter(|u| !u.trim().is_empty()).count();
                    if key.is_empty() && urls == 0 {
                        errors.push(format!(
                            "target \"{name}\" needs Apprise URLs or a configuration key"
                        ));
                    }
                    if !key.is_empty() && urls > 0 {
                        errors.push(format!(
                            "target \"{name}\": use Apprise URLs or a configuration key, not both"
                        ));
                    }
                    if !key.is_empty()
                        && (key.len() > 128
                            || !key
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                    {
                        errors.push(format!(
                            "target \"{name}\": a configuration key is 1-128 letters, digits, _ and -"
                        ));
                    }
                    let templates = [("title", &t.title), ("body", &t.body)]
                        .into_iter()
                        .chain(t.headers.iter().map(|h| ("header", &h.value)));
                    for (what, template) in templates {
                        if let Err(e) = crate::template::check(template) {
                            errors.push(format!("target \"{name}\": {what} template: {e}"));
                        }
                    }
                    check_headers(&mut errors, name, &t.headers);
                }
                NodeKind::Http(t) => {
                    check_target_common(&mut errors, name, Target::Http(t));
                    if t.url.trim().is_empty() {
                        errors.push(format!("target \"{name}\" needs a URL"));
                    }
                    if reqwest::Method::from_bytes(t.method.as_bytes()).is_err() {
                        errors.push(format!(
                            "target \"{name}\": {} is not an HTTP method",
                            t.method
                        ));
                    }
                    let templates = [("URL", &t.url), ("body", &t.body)]
                        .into_iter()
                        .chain(t.headers.iter().map(|h| ("header", &h.value)));
                    for (what, template) in templates {
                        if let Err(e) = crate::template::check(template) {
                            errors.push(format!("target \"{name}\": {what} template: {e}"));
                        }
                    }
                    check_headers(&mut errors, name, &t.headers);
                }
            }
        }
        for edge in &self.edges {
            let (Some(from), Some(to)) =
                (ids.get(edge.source.as_str()), ids.get(edge.target.as_str()))
            else {
                errors.push(format!(
                    "edge {} connects a node that doesn't exist",
                    edge.id
                ));
                continue;
            };
            let handle = edge.source_handle.as_deref();
            let ok = match (from, to) {
                (_, NodeKind::Source(_)) => false,
                (NodeKind::Source(_), _) => handle.is_none() || handle == Some(HANDLE_MATCH),
                (NodeKind::Filter(_), _) => {
                    matches!(handle, None | Some(HANDLE_MATCH | HANDLE_ELSE))
                }
                (NodeKind::Dedup(_), _) => {
                    matches!(handle, None | Some(HANDLE_NEW | HANDLE_DUPLICATE))
                }
                (NodeKind::Group(_), to) => handle.is_none() && to.delivers(),
                (NodeKind::Http(_) | NodeKind::Apprise(_), to) => {
                    to.delivers() && handle == Some(HANDLE_FALLBACK)
                }
            };
            if !ok {
                errors.push(format!(
                    "can't connect {} \"{}\" to {} \"{}\"",
                    from.label(),
                    from.name(),
                    to.label(),
                    to.name()
                ));
            }
        }
        errors
    }
}

fn check_target_common(errors: &mut Vec<String>, name: &str, t: Target<'_>) {
    if t.retry().max_attempts == 0 {
        errors.push(format!("target \"{name}\": attempts must be at least 1"));
    }
    if t.timeout_secs() == 0 {
        errors.push(format!(
            "target \"{name}\": the timeout must be at least 1 second"
        ));
    }
    for e in t.quiet_hours().check() {
        errors.push(format!("target \"{name}\": {e}"));
    }
}

fn check_headers(errors: &mut Vec<String>, name: &str, headers: &[Header]) {
    for h in headers {
        if reqwest::header::HeaderName::from_bytes(h.name.as_bytes()).is_err() {
            errors.push(format!(
                "target \"{name}\": \"{}\" is not a header name",
                h.name
            ));
        }
    }
}

impl NodeKind {
    pub fn name(&self) -> &str {
        match self {
            NodeKind::Source(s) => &s.name,
            NodeKind::Filter(f) => &f.name,
            NodeKind::Dedup(d) => &d.name,
            NodeKind::Group(g) => &g.name,
            NodeKind::Http(t) => &t.name,
            NodeKind::Apprise(t) => &t.name,
        }
    }

    pub fn as_target(&self) -> Option<Target<'_>> {
        match self {
            NodeKind::Http(t) => Some(Target::Http(t)),
            NodeKind::Apprise(t) => Some(Target::Apprise(t)),
            _ => None,
        }
    }

    /// A target, or a group of them: what a group or a fallback edge may lead to.
    fn delivers(&self) -> bool {
        matches!(self, NodeKind::Group(_)) || self.as_target().is_some()
    }

    fn label(&self) -> &'static str {
        match self {
            NodeKind::Source(_) => "source",
            NodeKind::Filter(_) => "filter",
            NodeKind::Dedup(_) => "dedupe",
            NodeKind::Group(_) => "group",
            NodeKind::Http(_) | NodeKind::Apprise(_) => "target",
        }
    }
}

impl FilterNode {
    pub fn matches(&self, n: &Notification) -> bool {
        let mut results = self.conditions.iter().map(|c| c.matches(n));
        match self.mode {
            MatchMode::All => results.all(|r| r),
            MatchMode::Any => self.conditions.is_empty() || results.any(|r| r),
        }
    }
}

impl Condition {
    fn check(&self) -> Result<(), String> {
        let known = matches!(
            self.field.as_str(),
            "title" | "body" | "priority" | "tags" | "source" | "url"
        ) || self
            .field
            .strip_prefix("data.")
            .is_some_and(|p| !p.is_empty());
        if !known {
            return Err(format!("unknown field \"{}\"", self.field));
        }
        if self.op == Op::Matches {
            let pattern = self
                .value
                .as_str()
                .ok_or("matches needs a regular expression")?;
            Regex::new(pattern).map_err(|e| format!("bad regular expression: {e}"))?;
        }
        if matches!(self.op, Op::Gt | Op::Gte | Op::Lt | Op::Lte) && self.value.as_f64().is_none() {
            return Err(format!("{} needs a number", self.field));
        }
        Ok(())
    }

    pub fn matches(&self, n: &Notification) -> bool {
        let actual = n.field(&self.field);
        match self.op {
            Op::Exists => actual.as_ref().is_some_and(|v| !v.is_null()),
            Op::NotExists => actual.as_ref().is_none_or(|v| v.is_null()),
            Op::Eq => actual.is_some_and(|v| loosely_equal(&v, &self.value)),
            Op::Ne => !actual.is_some_and(|v| loosely_equal(&v, &self.value)),
            Op::Contains => actual.is_some_and(|v| contains(&v, &self.value)),
            Op::NotContains => !actual.is_some_and(|v| contains(&v, &self.value)),
            Op::Matches => {
                let (Some(v), Some(pattern)) = (actual, self.value.as_str()) else {
                    return false;
                };
                Regex::new(pattern).is_ok_and(|re| re.is_match(&as_text(&v)))
            }
            Op::Gt | Op::Gte | Op::Lt | Op::Lte => {
                let (Some(a), Some(b)) = (actual.as_ref().and_then(as_number), self.value.as_f64())
                else {
                    return false;
                };
                match self.op {
                    Op::Gt => a > b,
                    Op::Gte => a >= b,
                    Op::Lt => a < b,
                    _ => a <= b,
                }
            }
        }
    }
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn as_number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Equal, treating "4" and 4 as the same and ignoring case in strings.
fn loosely_equal(a: &Value, b: &Value) -> bool {
    match (as_number(a), as_number(b)) {
        (Some(x), Some(y)) if !a.is_string() || !b.is_string() => x == y,
        _ => as_text(a).eq_ignore_ascii_case(&as_text(b)),
    }
}

/// A list contains an element equal to `needle`; anything else contains it as text, ignoring case.
fn contains(haystack: &Value, needle: &Value) -> bool {
    match haystack {
        Value::Array(items) => items.iter().any(|i| loosely_equal(i, needle)),
        other => as_text(other)
            .to_lowercase()
            .contains(&as_text(needle).to_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn note(priority: i64, tags: &[&str]) -> Notification {
        Notification {
            id: "n1".into(),
            source_id: "src".into(),
            source_name: "Tramline".into(),
            received_at: 0,
            title: Some("Print finished".into()),
            body: Some("part.gcode took 2h".into()),
            priority,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            url: None,
            data: json!({"printer": {"bed": 80}}),
        }
    }

    fn graph() -> Graph {
        serde_json::from_value(json!({
            "nodes": [
                {"id": "src", "type": "source", "position": {"x": 0, "y": 0}, "data": {"name": "Tramline"}},
                {"id": "hi", "type": "filter", "data": {"name": "High", "conditions": [
                    {"field": "priority", "op": "gte", "value": 4}
                ]}},
                {"id": "discord", "type": "http", "data": {"name": "Discord", "url": "https://example.com"}},
                {"id": "monitor", "type": "http", "data": {"name": "Monitor", "url": "http://monitor/display"}},
                {"id": "log", "type": "http", "data": {"name": "Log", "url": "http://log"}}
            ],
            "edges": [
                {"id": "e1", "source": "src", "target": "hi"},
                {"id": "e2", "source": "hi", "target": "discord", "sourceHandle": "match"},
                {"id": "e3", "source": "hi", "target": "monitor", "sourceHandle": "else"},
                {"id": "e4", "source": "src", "target": "log"},
                {"id": "e5", "source": "discord", "target": "monitor", "sourceHandle": "fallback"}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn routes_by_filter() {
        let g = graph();
        assert!(g.validate().is_empty(), "{:?}", g.validate());
        let route = |n| g.route("src", &n, |_| false).targets;
        assert_eq!(route(note(5, &[])), ["log", "discord"]);
        assert_eq!(route(note(2, &[])), ["log", "monitor"]);
        assert_eq!(g.fallbacks("discord"), ["monitor"]);
        assert!(g.fallbacks("monitor").is_empty());
    }

    #[test]
    fn conditions() {
        let n = note(3, &["print", "done"]);
        let c = |field: &str, op: Op, value: Value| {
            Condition {
                field: field.into(),
                op,
                value,
            }
            .matches(&n)
        };
        assert!(c("tags", Op::Contains, json!("PRINT")));
        assert!(!c("tags", Op::Contains, json!("fail")));
        assert!(c("title", Op::Contains, json!("finished")));
        assert!(c("title", Op::Matches, json!("^Print")));
        assert!(c("data.printer.bed", Op::Eq, json!(80)));
        assert!(c("data.printer.bed", Op::Gte, json!(70)));
        assert!(c("data.nozzle", Op::NotExists, Value::Null));
        assert!(c("source", Op::Eq, json!("tramline")));
        assert!(c("priority", Op::Eq, json!("3")));
    }

    #[test]
    fn rejects_bad_edges() {
        let mut g = graph();
        g.edges.push(Edge {
            id: "bad".into(),
            source: "monitor".into(),
            target: "log".into(),
            source_handle: None,
        });
        g.edges.push(Edge {
            id: "bad2".into(),
            source: "hi".into(),
            target: "src".into(),
            source_handle: None,
        });
        assert_eq!(g.validate().len(), 2);
    }

    #[test]
    fn routes_through_dedupe_and_apprise() {
        let g: Graph = serde_json::from_value(json!({
            "nodes": [
                {"id": "src", "type": "source", "data": {"name": "Tramline"}},
                {"id": "dd", "type": "dedup", "data": {"name": "Once", "windowSecs": 60}},
                {"id": "phone", "type": "apprise", "data": {"name": "Phone", "url": "http://apprise:8000", "urls": ["ntfy://topic"]}},
                {"id": "log", "type": "http", "data": {"name": "Log", "url": "http://log"}}
            ],
            "edges": [
                {"id": "e1", "source": "src", "target": "dd"},
                {"id": "e2", "source": "dd", "target": "phone", "sourceHandle": "new"},
                {"id": "e3", "source": "dd", "target": "log", "sourceHandle": "duplicate"},
                {"id": "e4", "source": "phone", "target": "log", "sourceHandle": "fallback"}
            ]
        }))
        .unwrap();
        assert!(g.validate().is_empty(), "{:?}", g.validate());
        let n = note(3, &[]);
        let first = g.route("src", &n, |_| false);
        assert_eq!(first.targets, ["phone"]);
        assert_eq!(first.passed, ["dd"]);
        let again = g.route("src", &n, |id| id == "dd");
        assert_eq!(again.targets, ["log"]);
        assert_eq!(again.duplicates, ["dd"]);
        assert_eq!(g.fallbacks("phone"), ["log"]);

        let mut bad = g.clone();
        if let NodeKind::Apprise(t) = &mut bad.nodes[2].kind {
            t.key = "has space".into();
        }
        bad.edges.push(Edge {
            id: "e5".into(),
            source: "dd".into(),
            target: "log".into(),
            source_handle: Some("match".into()),
        });
        // Both URLs and a key, a bad key, and a filter handle on a dedupe node.
        assert_eq!(bad.validate().len(), 3, "{:?}", bad.validate());
    }

    #[test]
    fn routes_through_groups() {
        let g: Graph = serde_json::from_value(json!({
            "nodes": [
                {"id": "src", "type": "source", "data": {"name": "Tramline"}},
                {"id": "hi", "type": "filter", "data": {"name": "High", "conditions": [
                    {"field": "priority", "op": "gte", "value": 4}
                ]}},
                {"id": "phones", "type": "group", "data": {"name": "Phones"}},
                {"id": "everyone", "type": "group", "data": {"name": "Everyone"}},
                {"id": "a", "type": "http", "data": {"name": "A", "url": "http://a"}},
                {"id": "b", "type": "http", "data": {"name": "B", "url": "http://b"}},
                {"id": "c", "type": "http", "data": {"name": "C", "url": "http://c"}},
                {"id": "d", "type": "http", "data": {"name": "D", "url": "http://d"}}
            ],
            "edges": [
                {"id": "e1", "source": "src", "target": "hi"},
                {"id": "e2", "source": "hi", "target": "phones", "sourceHandle": "match"},
                {"id": "e3", "source": "hi", "target": "everyone", "sourceHandle": "else"},
                {"id": "e4", "source": "phones", "target": "a"},
                {"id": "e5", "source": "phones", "target": "b"},
                {"id": "e6", "source": "everyone", "target": "phones"},
                {"id": "e7", "source": "everyone", "target": "c"},
                {"id": "e8", "source": "phones", "target": "everyone"},
                {"id": "e9", "source": "d", "target": "phones", "sourceHandle": "fallback"},
                {"id": "e10", "source": "a", "target": "everyone", "sourceHandle": "fallback"}
            ]
        }))
        .unwrap();
        assert!(g.validate().is_empty(), "{:?}", g.validate());
        let route = |n| g.route("src", &n, |_| false).targets;
        // Groups that contain each other still reach each target once.
        assert_eq!(route(note(5, &[])), ["a", "b", "c"]);
        assert_eq!(route(note(2, &[])), ["c", "a", "b"]);
        assert_eq!(g.fallbacks("d"), ["a", "b", "c"]);
        // A target never falls back to itself through a group.
        assert_eq!(g.fallbacks("a"), ["c", "b"]);

        let mut bad = g.clone();
        for (id, source, target, handle) in [
            ("x1", "phones", "hi", None), // a group leads only to targets and groups
            ("x2", "phones", "a", Some("fallback")), // and has no handles
        ] {
            bad.edges.push(Edge {
                id: id.into(),
                source: source.into(),
                target: target.into(),
                source_handle: handle.map(Into::into),
            });
        }
        assert_eq!(bad.validate().len(), 2, "{:?}", bad.validate());
    }
}
