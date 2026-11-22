use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::{Notify, broadcast, mpsc};
use webauthn_rs::Webauthn;

use crate::api::auth::Ceremony;
use crate::config::Config;
use crate::graph::Graph;

/// Something the GUI should refresh for, sent over `/api/events`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub kind: &'static str,
    pub notification_id: String,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub db: SqlitePool,
    graph: RwLock<Arc<Graph>>,
    pub http: reqwest::Client,
    /// Notification ids that are stored and waiting to be routed.
    pub route_tx: mpsc::UnboundedSender<String>,
    /// Wakes the delivery worker when a delivery becomes due or a slot frees up.
    pub wake: Notify,
    pub events: broadcast::Sender<Event>,
    pub webauthn: Webauthn,
    /// Passkey sign-ins and registrations in progress, by ceremony id.
    pub ceremonies: Mutex<HashMap<String, Ceremony>>,
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    pub fn new(
        config: Config,
        db: SqlitePool,
        graph: Graph,
        route_tx: mpsc::UnboundedSender<String>,
    ) -> anyhow::Result<Self> {
        let webauthn = crate::api::auth::webauthn(&config.origin)?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("switchboard/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("HTTP client");
        Ok(Self(Arc::new(Inner {
            config,
            db,
            graph: RwLock::new(Arc::new(graph)),
            http,
            route_tx,
            wake: Notify::new(),
            events: broadcast::channel(256).0,
            webauthn,
            ceremonies: Mutex::new(HashMap::new()),
        })))
    }

    pub fn graph(&self) -> Arc<Graph> {
        self.graph.read().expect("graph lock").clone()
    }

    pub fn set_graph(&self, graph: Graph) {
        *self.graph.write().expect("graph lock") = Arc::new(graph);
    }

    pub fn emit(&self, kind: &'static str, notification_id: &str) {
        // No GUI listening is fine.
        let _ = self.events.send(Event {
            kind,
            notification_id: notification_id.to_string(),
        });
    }
}
