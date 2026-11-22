//! The routing graph: what the GUI draws with React Flow, and what the engine follows.
//!
//! The graph is stored exactly as the GUI sends it (positions and all), and parsed into these
//! types for validation and routing. Fields the engine doesn't know, such as `position`, are ignored.
//!
//! - `source` nodes are where notifications come in. Each has its own token.
//! - `filter` nodes test a notification. Edges leave by the `match` handle (or no handle) when it
//!   passes, and by the `else` handle when it doesn't.
//! - `http` nodes are targets. Edges leaving by the `fallback` handle go to the targets tried
//!   once this target has given up.

use std::collections::{HashMap, HashSet, VecDeque};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::notification::Notification;

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
    Http(HttpTarget),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceNode {
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

    pub fn target(&self, id: &str) -> Option<&HttpTarget> {
        match self.node(id).map(|n| &n.kind) {
            Some(NodeKind::Http(t)) => Some(t),
            _ => None,
        }
    }

    fn edges_from<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Edge> + 'a {
        self.edges.iter().filter(move |e| e.source == id)
    }

    /// The targets a notification from `source_id` goes to, in graph order, each once.
    pub fn route(&self, source_id: &str, n: &Notification) -> Vec<String> {
        let mut targets = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([source_id.to_string()]);
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(node) = self.node(&id) else { continue };
            let follow: Option<&str> = match &node.kind {
                NodeKind::Source(_) => None,
                NodeKind::Filter(f) => Some(if f.matches(n) {
                    HANDLE_MATCH
                } else {
                    HANDLE_ELSE
                }),
                NodeKind::Http(_) => {
                    targets.push(id.clone());
                    continue; // only fallback edges leave a target; the engine follows those
                }
            };
            for edge in self.edges_from(&id) {
                let handle = edge.source_handle.as_deref().unwrap_or(HANDLE_MATCH);
                if follow.is_none_or(|h| h == handle) {
                    queue.push_back(edge.target.clone());
                }
            }
        }
        targets
    }

    /// The targets to try once `target_id` has given up.
    pub fn fallbacks(&self, target_id: &str) -> Vec<String> {
        self.edges_from(target_id)
            .filter(|e| e.source_handle.as_deref() == Some(HANDLE_FALLBACK))
            .filter(|e| self.target(&e.target).is_some())
            .map(|e| e.target.clone())
            .collect()
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
                NodeKind::Source(_) => {}
                NodeKind::Filter(f) => {
                    for c in &f.conditions {
                        if let Err(e) = c.check() {
                            errors.push(format!("filter \"{name}\": {e}"));
                        }
                    }
                }
                NodeKind::Http(t) => {
                    if t.url.trim().is_empty() {
                        errors.push(format!("target \"{name}\" needs a URL"));
                    }
                    if reqwest::Method::from_bytes(t.method.as_bytes()).is_err() {
                        errors.push(format!(
                            "target \"{name}\": {} is not an HTTP method",
                            t.method
                        ));
                    }
                    if t.retry.max_attempts == 0 {
                        errors.push(format!("target \"{name}\": attempts must be at least 1"));
                    }
                    if t.timeout_secs == 0 {
                        errors.push(format!(
                            "target \"{name}\": the timeout must be at least 1 second"
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
                    for h in &t.headers {
                        if reqwest::header::HeaderName::from_bytes(h.name.as_bytes()).is_err() {
                            errors.push(format!(
                                "target \"{name}\": \"{}\" is not a header name",
                                h.name
                            ));
                        }
                    }
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
                (NodeKind::Http(_), NodeKind::Http(_)) => handle == Some(HANDLE_FALLBACK),
                (NodeKind::Http(_), _) => false,
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

impl NodeKind {
    pub fn name(&self) -> &str {
        match self {
            NodeKind::Source(s) => &s.name,
            NodeKind::Filter(f) => &f.name,
            NodeKind::Http(t) => &t.name,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            NodeKind::Source(_) => "source",
            NodeKind::Filter(_) => "filter",
            NodeKind::Http(_) => "target",
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
        assert_eq!(g.route("src", &note(5, &[])), ["log", "discord"]);
        assert_eq!(g.route("src", &note(2, &[])), ["log", "monitor"]);
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
}
