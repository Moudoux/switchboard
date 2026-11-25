//! switchboard: one place for all projects to send notifications, which it stores and delivers onward.

mod api;
mod config;
mod db;
mod engine;
mod graph;
mod notification;
mod quiet_hours;
mod state;
mod template;
mod util;

use anyhow::Context;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

const USAGE: &str = "usage: switchboard [serve | enroll | health]

  serve    run the server (the default)
  enroll   print a one-time link to register a passkey for the GUI
  health   check that the server answers (for container health checks)

Settings come from the environment; see the README.";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "switchboard=info,tower_http=warn".into()),
        )
        .init();
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let config = Config::from_env()?;
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve(config).await,
        Some("enroll") => {
            let db = db::connect(&config.db_path).await?;
            api::auth::enroll_command(&db, &config.origin).await
        }
        Some("health") => health(&config.listen).await,
        Some(_) => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let db = db::connect(&config.db_path)
        .await
        .with_context(|| format!("opening {}", config.db_path.display()))?;
    let (version, graph) = db::load_graph(&db).await?;
    let graph = serde_json::from_value(graph).context("the saved graph doesn't parse")?;
    let (route_tx, route_rx) = mpsc::unbounded_channel();
    let listen = config.listen.clone();
    let state = AppState::new(config, db, graph, route_tx)?;

    tokio::spawn(engine::router::run(state.clone(), route_rx));
    tokio::spawn(engine::delivery::run(state.clone()));
    tokio::spawn(engine::cleanup(state.clone()));

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("listening on {listen}"))?;
    tracing::info!(
        graph_version = version,
        origin = state.config.origin,
        "listening on {listen}"
    );
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(shutdown())
        .await?;
    tracing::info!("stopped");
    Ok(())
}

/// Exits 0 when the server on this machine answers /healthz.
async fn health(listen: &str) -> anyhow::Result<()> {
    let port = listen.rsplit(':').next().unwrap_or("8080");
    let url = format!("http://127.0.0.1:{port}/healthz");
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;
    anyhow::ensure!(
        response.status().is_success(),
        "{url}: HTTP {}",
        response.status()
    );
    Ok(())
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {
        _ = ctrl_c => {}
        _ = term.recv() => {}
    }
}
