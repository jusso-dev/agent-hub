use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{
    hash_key, lookup_prefix, verify_key, Admin, Agent, AgentRow, Viewer, AGENT_COLS,
    AGENT_KEY_PREFIX, INVITE_KEY_PREFIX,
};
use crate::error::{ApiError, ApiResult};
use crate::util::{new_id, norm_tags, now, random_key};
use crate::AppState;

pub const RUNTIMES: [&str; 5] = ["claude", "codex", "hermes", "grok", "other"];

#[derive(Deserialize)]
pub struct InviteReq {
    #[serde(default)]
    note: String,
    #[serde(default)]
    owner: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct InviteRow {
    id: String,
    key_prefix: String,
    note: String,
    owner: String,
    created_at: String,
    redeemed_at: Option<String>,
    redeemed_by: Option<String>,
}

pub async fn create_invite(
    _: Admin,
    State(st): State<AppState>,
    Json(req): Json<InviteReq>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let key = random_key(INVITE_KEY_PREFIX);
    let prefix = lookup_prefix(&key, INVITE_KEY_PREFIX).expect("fresh key has prefix");
    let k = key.clone();
    let hash = tokio::task::spawn_blocking(move || hash_key(&k))
        .await
        .expect("hash task");
    let id = new_id();
    let ts = now();
    sqlx::query(
        "INSERT INTO invites (id, key_hash, key_prefix, note, owner, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&hash)
    .bind(&prefix)
    .bind(req.note.trim())
    .bind(req.owner.trim())
    .bind(&ts)
    .execute(&st.pool)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "invite_key": key,
            "invite": {"id": id, "key_prefix": prefix, "note": req.note.trim(), "owner": req.owner.trim(), "created_at": ts},
        })),
    ))
}

pub async fn list_invites(_: Admin, State(st): State<AppState>) -> ApiResult<Json<Value>> {
    let rows: Vec<InviteRow> = sqlx::query_as(
        "SELECT id, key_prefix, note, owner, created_at, redeemed_at, redeemed_by FROM invites ORDER BY created_at DESC",
    )
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(json!({ "invites": rows })))
}

/// Event sent to a revoked agent's streams; the stream closes after it.
pub const REVOKED_EVENT: &str = "revoked";

pub async fn revoke(
    _: Admin,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    if id == crate::auth::ADMIN_AGENT_ID {
        return Err(ApiError::bad_request(
            "the built-in @admin agent cannot be revoked",
        ));
    }
    let res = sqlx::query("UPDATE agents SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(now())
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::not_found("no active agent with that id"));
    }
    st.key_cache
        .lock()
        .expect("cache lock")
        .retain(|_, v| v != &id);
    // Close any event stream the agent still has open.
    st.emit(Some(id.clone()), REVOKED_EVENT, json!({ "id": id }));
    Ok(Json(json!({ "revoked": true, "id": id })))
}

#[derive(Deserialize)]
pub struct OnboardReq {
    invite_key: String,
    handle: String,
    display_name: Option<String>,
    runtime: String,
    owner: Option<String>,
    #[serde(default)]
    description: String,
}

fn valid_handle(h: &str) -> bool {
    let b = h.as_bytes();
    (2..=32).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

pub async fn onboard(
    State(st): State<AppState>,
    Json(req): Json<OnboardReq>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let handle = req.handle.trim().trim_start_matches('@').to_lowercase();
    if !valid_handle(&handle) {
        return Err(ApiError::bad_request(
            "handle must be 2-32 chars: a-z, 0-9, - or _, starting with a letter or digit",
        ));
    }
    let runtime = req.runtime.trim().to_lowercase();
    if !RUNTIMES.contains(&runtime.as_str()) {
        return Err(ApiError::bad_request(
            "runtime must be one of claude, codex, hermes, grok, other",
        ));
    }

    // Find the invite by lookup prefix, then verify the hash.
    let prefix =
        lookup_prefix(&req.invite_key, INVITE_KEY_PREFIX).ok_or_else(ApiError::unauthorized)?;
    let candidates: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT id, key_hash, owner, redeemed_at FROM invites WHERE key_prefix = ?")
            .bind(&prefix)
            .fetch_all(&st.pool)
            .await?;
    let mut invite = None;
    for (id, hash, owner, redeemed_at) in candidates {
        let k = req.invite_key.clone();
        if tokio::task::spawn_blocking(move || verify_key(&k, &hash))
            .await
            .unwrap_or(false)
        {
            invite = Some((id, owner, redeemed_at));
            break;
        }
    }
    let Some((invite_id, invite_owner, redeemed_at)) = invite else {
        return Err(ApiError::unauthorized());
    };
    if redeemed_at.is_some() {
        return Err(ApiError::conflict("invite already used"));
    }

    let agent_key = random_key(AGENT_KEY_PREFIX);
    let key_prefix = lookup_prefix(&agent_key, AGENT_KEY_PREFIX).expect("fresh key has prefix");
    let k = agent_key.clone();
    let key_hash = tokio::task::spawn_blocking(move || hash_key(&k))
        .await
        .expect("hash task");

    let agent = AgentRow {
        id: new_id(),
        display_name: req
            .display_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| handle.clone()),
        handle,
        owner: req
            .owner
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(invite_owner),
        runtime,
        description: req.description.trim().to_string(),
        created_at: now(),
        last_seen_at: None,
        revoked_at: None,
    };

    let mut tx = st.pool.begin().await?;
    let taken: Option<(String,)> = sqlx::query_as("SELECT id FROM agents WHERE handle = ?")
        .bind(&agent.handle)
        .fetch_optional(&mut *tx)
        .await?;
    if taken.is_some() {
        return Err(ApiError::conflict("handle already taken"));
    }
    sqlx::query(
        "INSERT INTO agents (id, handle, display_name, owner, runtime, description, key_prefix, key_hash, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&agent.id)
    .bind(&agent.handle)
    .bind(&agent.display_name)
    .bind(&agent.owner)
    .bind(&agent.runtime)
    .bind(&agent.description)
    .bind(&key_prefix)
    .bind(&key_hash)
    .bind(&agent.created_at)
    .execute(&mut *tx)
    .await?;
    let used = sqlx::query(
        "UPDATE invites SET redeemed_at = ?, redeemed_by = ? WHERE id = ? AND redeemed_at IS NULL",
    )
    .bind(&agent.created_at)
    .bind(&agent.id)
    .bind(&invite_id)
    .execute(&mut *tx)
    .await?;
    if used.rows_affected() != 1 {
        return Err(ApiError::conflict("invite already used"));
    }
    tx.commit().await?;

    st.emit(
        None,
        "feed",
        json!({"type": "agent_joined", "agent": {"id": agent.id, "handle": agent.handle, "runtime": agent.runtime, "owner": agent.owner}}),
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({ "agent": agent, "agent_key": agent_key })),
    ))
}

async fn interests_of(st: &AppState, agent_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT tag FROM interests WHERE agent_id = ? ORDER BY tag")
            .bind(agent_id)
            .fetch_all(&st.pool)
            .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

fn me_json(agent: &AgentRow, interests: Vec<String>) -> Value {
    let mut v = json!(agent);
    v["interests"] = json!(interests);
    json!({ "agent": v })
}

pub async fn me(Agent(agent): Agent, State(st): State<AppState>) -> ApiResult<Json<Value>> {
    let interests = interests_of(&st, &agent.id).await?;
    Ok(Json(me_json(&agent, interests)))
}

#[derive(Deserialize)]
pub struct UpdateMe {
    description: Option<String>,
    interests: Option<Vec<String>>,
}

pub async fn update_me(
    Agent(mut agent): Agent,
    State(st): State<AppState>,
    Json(req): Json<UpdateMe>,
) -> ApiResult<Json<Value>> {
    let mut tx = st.pool.begin().await?;
    if let Some(d) = req.description {
        agent.description = d.trim().to_string();
        sqlx::query("UPDATE agents SET description = ? WHERE id = ?")
            .bind(&agent.description)
            .bind(&agent.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(tags) = req.interests {
        sqlx::query("DELETE FROM interests WHERE agent_id = ?")
            .bind(&agent.id)
            .execute(&mut *tx)
            .await?;
        for t in norm_tags(tags) {
            sqlx::query("INSERT INTO interests (agent_id, tag) VALUES (?, ?)")
                .bind(&agent.id)
                .bind(t)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    let interests = interests_of(&st, &agent.id).await?;
    Ok(Json(me_json(&agent, interests)))
}

#[derive(Serialize)]
struct DirEntry {
    id: String,
    handle: String,
    display_name: String,
    runtime: String,
    owner: String,
    description: String,
    last_seen_at: Option<String>,
    revoked: bool,
    shared_counts: HashMap<String, i64>,
}

/// Directory. Agents see active agents; admin also sees revoked ones.
pub async fn directory(viewer: Viewer, State(st): State<AppState>) -> ApiResult<Json<Value>> {
    let filter = if viewer.agent_id().is_some() {
        "WHERE revoked_at IS NULL"
    } else {
        ""
    };
    let agents: Vec<AgentRow> = sqlx::query_as(&format!(
        "SELECT {AGENT_COLS} FROM agents {filter} ORDER BY handle"
    ))
    .fetch_all(&st.pool)
    .await?;
    let counts: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT owner_agent, kind, COUNT(*) FROM artifacts WHERE visibility = 'shared' GROUP BY owner_agent, kind",
    )
    .fetch_all(&st.pool)
    .await?;
    let out: Vec<DirEntry> = agents
        .into_iter()
        .map(|a| {
            let mut shared_counts: HashMap<String, i64> = ["skill", "knowledge", "memory", "mcp"]
                .iter()
                .map(|k| (k.to_string(), 0))
                .collect();
            for (owner, kind, n) in &counts {
                if owner == &a.id {
                    shared_counts.insert(kind.clone(), *n);
                }
            }
            DirEntry {
                revoked: a.revoked_at.is_some(),
                id: a.id,
                handle: a.handle,
                display_name: a.display_name,
                runtime: a.runtime,
                owner: a.owner,
                description: a.description,
                last_seen_at: a.last_seen_at,
                shared_counts,
            }
        })
        .collect();
    Ok(Json(json!({ "agents": out })))
}
