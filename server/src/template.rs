//! Templates for target URLs, headers and bodies (MiniJinja, Jinja2 syntax).
//!
//! A template sees the notification as `n` (also `notification`). `{{ n.title|tojson }}` gives a
//! JSON string; a missing `n.data.x.y` renders as nothing instead of failing.

use std::sync::LazyLock;

use minijinja::value::Serde;
use minijinja::{Environment, UndefinedBehavior, Value};

static ENV: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    // Bodies are JSON or plain text, never HTML.
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
    env
});

pub fn check(source: &str) -> Result<(), String> {
    ENV.template_from_str(source).map(|_| ()).map_err(describe)
}

pub fn render(source: &str, n: &serde_json::Value) -> Result<String, String> {
    let ctx =
        minijinja::context! { n => Value::from(Serde(n)), notification => Value::from(Serde(n)) };
    ENV.render_str(source, ctx).map_err(describe)
}

fn describe(e: minijinja::Error) -> String {
    match e.line() {
        Some(line) => format!(
            "{} (line {line})",
            e.detail().unwrap_or(&e.kind().to_string())
        ),
        None => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn renders_json_and_tolerates_missing_data() {
        let n = json!({"title": "Hi \"you\"", "priority": 4, "data": {}});
        let out = super::render(r#"{"t": {{ n.title|tojson }}, "c": {{ [1,2,3,4,5][n.priority - 1] }}, "x": "{{ n.data.a.b }}"}"#, &n).unwrap();
        assert_eq!(out, r#"{"t": "Hi \"you\"", "c": 4, "x": ""}"#);
        assert!(super::check("{{ n.title").is_err());
    }
}
