use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// What a sender posts. Only a title or a body is required.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub title: Option<String>,
    #[serde(alias = "message")]
    pub body: Option<String>,
    /// 1 (lowest) to 5 (highest); 3 when left out.
    pub priority: Option<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// A link the notification is about.
    pub url: Option<String>,
    /// Anything else, for filters and templates.
    #[serde(default)]
    pub data: Map<String, Value>,
}

pub const MAX_TITLE: usize = 1_000;
pub const MAX_BODY: usize = 32_000;

impl Input {
    pub fn validate(&self) -> Result<(), String> {
        let blank = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
        if blank(&self.title) && blank(&self.body) {
            return Err("a notification needs a title or a body".into());
        }
        if self.title.as_ref().is_some_and(|t| t.len() > MAX_TITLE) {
            return Err(format!("title is longer than {MAX_TITLE} bytes"));
        }
        if self.body.as_ref().is_some_and(|b| b.len() > MAX_BODY) {
            return Err(format!("body is longer than {MAX_BODY} bytes"));
        }
        if let Some(p) = self.priority
            && !(1..=5).contains(&p)
        {
            return Err("priority must be 1 to 5".into());
        }
        if self.tags.len() > 50 {
            return Err("at most 50 tags".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    pub source_id: String,
    pub source_name: String,
    pub received_at: i64,
    pub title: Option<String>,
    pub body: Option<String>,
    pub priority: i64,
    #[sqlx(json)]
    pub tags: Vec<String>,
    pub url: Option<String>,
    #[sqlx(json)]
    pub data: Value,
}

impl Notification {
    /// A field by the name filters use: `title`, `priority`, `data.printer.bed`, ...
    pub fn field(&self, name: &str) -> Option<Value> {
        match name {
            "title" => self.title.clone().map(Value::String),
            "body" => self.body.clone().map(Value::String),
            "priority" => Some(self.priority.into()),
            "tags" => Some(json!(self.tags)),
            "source" => Some(self.source_name.clone().into()),
            "url" => self.url.clone().map(Value::String),
            other => {
                let path = other.strip_prefix("data.")?;
                path.split('.')
                    .try_fold(&self.data, |v, key| v.get(key))
                    .cloned()
            }
        }
    }

    /// What templates see as `n` (and `notification`).
    pub fn template_value(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "body": self.body,
            "text": self.text(),
            "priority": self.priority,
            "tags": self.tags,
            "url": self.url,
            "data": self.data,
            "source": {"id": self.source_id, "name": self.source_name},
            "receivedAt": crate::util::rfc3339(self.received_at),
            "receivedAtMs": self.received_at,
        })
    }

    /// The body, or the title when there is no body.
    pub fn text(&self) -> String {
        self.body
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_default()
    }
}
