pub mod auth;
pub mod db;
pub mod error;
pub mod routes;
pub mod util;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::DefaultBodyLimit;
use axum::Router;
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tower_http::trace::TraceLayer;

/// Max request body: 256 KB.
pub const BODY_LIMIT: usize = 256 * 1024;
/// Requests allowed per key per minute.
pub const RATE_LIMIT_PER_MIN: u32 = 60;

/// Something pushed to SSE subscribers.
#[derive(Clone, Debug, Serialize)]
pub struct HubEvent {
    /// `None` means every connected client.
    #[serde(skip)]
    pub target: Option<String>,
    /// `inbox` or `feed`.
    pub event: &'static str,
    pub data: serde_json::Value,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub admin_key: Arc<String>,
    pub events: broadcast::Sender<HubEvent>,
    /// Verified bearer token -> agent id, so argon2 runs once per key per process.
    pub key_cache: Arc<Mutex<HashMap<String, String>>>,
    /// key id -> (window start, count).
    pub rate: Arc<Mutex<HashMap<String, (std::time::Instant, u32)>>>,
    pub rate_limit_per_min: u32,
}

impl AppState {
    pub fn new(pool: SqlitePool, admin_key: String) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            pool,
            admin_key: Arc::new(admin_key),
            events,
            key_cache: Arc::default(),
            rate: Arc::default(),
            rate_limit_per_min: RATE_LIMIT_PER_MIN,
        }
    }

    pub fn emit(&self, target: Option<String>, event: &'static str, data: serde_json::Value) {
        // No subscribers is fine.
        let _ = self.events.send(HubEvent {
            target,
            event,
            data,
        });
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .nest("/api", routes::api())
        .fallback(routes::web::serve)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
