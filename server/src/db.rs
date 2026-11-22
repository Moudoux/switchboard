use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::graph::Graph;

pub async fn connect(path: &Path) -> anyhow::Result<SqlitePool> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let options = SqliteConnectOptions::from_str("sqlite:")?
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        // With WAL, NORMAL keeps every commit across a crash of the process, and loses at most
        // the last ones on a power cut; the sender has its 202 either way.
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(10))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

/// The newest saved graph, as the GUI sent it, with its version; an empty graph before the first save.
pub async fn load_graph(db: &SqlitePool) -> anyhow::Result<(i64, serde_json::Value)> {
    let row: Option<(i64, String)> =
        sqlx::query_as("SELECT id, graph FROM graph_versions ORDER BY id DESC LIMIT 1")
            .fetch_optional(db)
            .await?;
    Ok(match row {
        Some((id, json)) => (id, serde_json::from_str(&json)?),
        None => (0, serde_json::to_value(Graph::default())?),
    })
}

pub async fn setting(db: &SqlitePool, key: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await
}

pub async fn set_setting(db: &SqlitePool, key: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = ?2")
        .bind(key)
        .bind(value)
        .execute(db)
        .await
        .map(|_| ())
}
