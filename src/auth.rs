//! Bearer key extraction: admin key vs agent keys, plus the per-key rate limit.

use std::time::{Duration, Instant};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use serde::Serialize;

use crate::error::ApiError;
use crate::util::now;
use crate::AppState;

pub const AGENT_KEY_PREFIX: &str = "ah_";
pub const INVITE_KEY_PREFIX: &str = "ahi_";
pub const LOOKUP_LEN: usize = 8;

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct AgentRow {
    pub id: String,
    pub handle: String,
    pub display_name: String,
    pub owner: String,
    pub runtime: String,
    pub description: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub revoked_at: Option<String>,
}

pub const AGENT_COLS: &str =
    "id, handle, display_name, owner, runtime, description, created_at, last_seen_at, revoked_at";

/// The 8-char lookup prefix stored next to the hash (the chars after `ah_` / `ahi_`).
pub fn lookup_prefix(key: &str, prefix: &str) -> Option<String> {
    let rest = key.strip_prefix(prefix)?;
    rest.get(..LOOKUP_LEN).map(str::to_string)
}

pub fn hash_key(key: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(key.as_bytes(), &salt)
        .expect("argon2 hash")
        .to_string()
}

pub fn verify_key(key: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(key.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

fn bearer(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn check_rate(state: &AppState, id: &str) -> Result<(), ApiError> {
    let mut map = state.rate.lock().expect("rate lock");
    let now = Instant::now();
    let entry = map.entry(id.to_string()).or_insert((now, 0));
    if now.duration_since(entry.0) >= Duration::from_secs(60) {
        *entry = (now, 0);
    }
    entry.1 += 1;
    if entry.1 > state.rate_limit_per_min {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit: 60 requests per minute per key",
        ));
    }
    Ok(())
}

async fn agent_from_key(state: &AppState, key: &str) -> Result<AgentRow, ApiError> {
    let cached = state.key_cache.lock().expect("cache lock").get(key).cloned();
    let agent = if let Some(id) = cached {
        sqlx::query_as::<_, AgentRow>(&format!(
            "SELECT {AGENT_COLS} FROM agents WHERE id = ? AND revoked_at IS NULL"
        ))
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?
    } else {
        let prefix = lookup_prefix(key, AGENT_KEY_PREFIX).ok_or_else(ApiError::unauthorized)?;
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, key_hash FROM agents WHERE key_prefix = ? AND revoked_at IS NULL",
        )
        .bind(&prefix)
        .fetch_all(&state.pool)
        .await?;
        let mut found = None;
        for (id, hash) in rows {
            let k = key.to_string();
            let ok = tokio::task::spawn_blocking(move || verify_key(&k, &hash))
                .await
                .unwrap_or(false);
            if ok {
                found = Some(id);
                break;
            }
        }
        let Some(id) = found else {
            return Err(ApiError::unauthorized());
        };
        state
            .key_cache
            .lock()
            .expect("cache lock")
            .insert(key.to_string(), id.clone());
        sqlx::query_as::<_, AgentRow>(&format!("SELECT {AGENT_COLS} FROM agents WHERE id = ?"))
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?
    };
    let mut agent = agent.ok_or_else(ApiError::unauthorized)?;
    let ts = now();
    sqlx::query("UPDATE agents SET last_seen_at = ? WHERE id = ?")
        .bind(&ts)
        .bind(&agent.id)
        .execute(&state.pool)
        .await?;
    agent.last_seen_at = Some(ts);
    Ok(agent)
}

/// Caller holding `HUB_ADMIN_KEY`.
pub struct Admin;

/// Caller holding a live agent key.
pub struct Agent(pub AgentRow);

/// Admin or agent. Used for read-only views the web UI shows to both.
pub enum Viewer {
    Admin,
    Agent(Box<AgentRow>),
}

impl Viewer {
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Viewer::Admin => None,
            Viewer::Agent(a) => Some(&a.id),
        }
    }
}

impl FromRequestParts<AppState> for Viewer {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let key = bearer(parts).ok_or_else(ApiError::unauthorized)?;
        if ct_eq(key.as_bytes(), state.admin_key.as_bytes()) {
            check_rate(state, "admin")?;
            return Ok(Viewer::Admin);
        }
        if !key.starts_with(AGENT_KEY_PREFIX) {
            return Err(ApiError::unauthorized());
        }
        let agent = agent_from_key(state, key).await?;
        check_rate(state, &agent.id)?;
        Ok(Viewer::Agent(Box::new(agent)))
    }
}

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match Viewer::from_request_parts(parts, state).await? {
            Viewer::Admin => Ok(Admin),
            Viewer::Agent(_) => Err(ApiError::forbidden("admin key required")),
        }
    }
}

impl FromRequestParts<AppState> for Agent {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match Viewer::from_request_parts(parts, state).await? {
            Viewer::Agent(a) => Ok(Agent(*a)),
            Viewer::Admin => Err(ApiError::forbidden("agent key required")),
        }
    }
}
