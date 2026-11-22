use std::env;
use std::path::PathBuf;

/// Settings from the environment; see the README for the list.
#[derive(Debug, Clone)]
pub struct Config {
    pub listen: String,
    pub db_path: PathBuf,
    pub web_dir: PathBuf,
    pub retention_days: i64,
    pub concurrency: usize,
    pub cookie_secure: bool,
    /// Where the GUI is opened, such as `https://notify.devops.lan`. Passkeys are bound to its host.
    pub origin: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        fn var(name: &str, default: &str) -> String {
            env::var(name).unwrap_or_else(|_| default.to_string())
        }
        fn parse<T: std::str::FromStr>(name: &str, default: &str) -> anyhow::Result<T> {
            var(name, default)
                .parse()
                .map_err(|_| anyhow::anyhow!("{name} is not valid"))
        }
        Ok(Self {
            listen: var("SWITCHBOARD_LISTEN", "0.0.0.0:8080"),
            db_path: var("SWITCHBOARD_DB", "data/switchboard.db").into(),
            web_dir: var("SWITCHBOARD_WEB_DIR", "web/dist").into(),
            retention_days: parse("SWITCHBOARD_RETENTION_DAYS", "30")?,
            concurrency: parse::<usize>("SWITCHBOARD_CONCURRENCY", "16")?.max(1),
            cookie_secure: parse("SWITCHBOARD_COOKIE_SECURE", "true")?,
            origin: var("SWITCHBOARD_ORIGIN", "http://localhost:8080")
                .trim_end_matches('/')
                .to_string(),
        })
    }
}
