use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{Agent, Viewer};
use crate::error::ApiResult;
use crate::routes::artifacts::{
    changed_since, leaks_secret, secret_error, suggestions_for, upsert_artifact, validate, TagsIn,
    UpsertInput,
};
use crate::routes::messages::channel_messages_since;
use crate::util::now;
use crate::AppState;

#[derive(Deserialize)]
pub struct SyncItem {
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    tags: TagsIn,
    content: String,
    visibility: Option<String>,
}

#[derive(Deserialize)]
pub struct SyncReq {
    #[serde(default)]
    memories: Vec<SyncItem>,
    #[serde(default)]
    skills: Vec<SyncItem>,
    #[serde(default)]
    mcps: Vec<SyncItem>,
}

/// Periodic update: upsert every item, report what changed, suggest things to learn.
/// Nothing is written if any item is invalid, or if any MCP config or shared
/// item holds a secret.
pub async fn sync(
    Agent(me): Agent,
    State(st): State<AppState>,
    Json(req): Json<SyncReq>,
) -> ApiResult<Json<Value>> {
    let mut inputs = Vec::new();
    for (kind, items) in [
        ("memory", req.memories),
        ("skill", req.skills),
        ("mcp", req.mcps),
    ] {
        for it in items {
            inputs.push(UpsertInput {
                kind: kind.to_string(),
                name: it.name,
                summary: it.summary.trim().to_string(),
                tags: it.tags.norm(),
                content: it.content,
                visibility: it.visibility,
            });
        }
    }
    // Name every leaky item at once, then check the rest.
    let leaky: Vec<String> = inputs
        .iter()
        .filter(|i| leaks_secret(i))
        .map(|i| i.name.clone())
        .collect();
    if !leaky.is_empty() {
        return Err(secret_error(&leaky));
    }
    for i in &inputs {
        validate(i)?;
    }

    let mut changed = Vec::new();
    let mut unchanged = 0;
    for input in inputs {
        let (art, did_change) = upsert_artifact(&st, &me, input).await?;
        if did_change {
            changed.push(
                json!({"id": art.id, "kind": art.kind, "name": art.name, "version": art.version}),
            );
        } else {
            unchanged += 1;
        }
    }
    let suggestions = suggestions_for(&st, &me.id).await?;
    Ok(Json(json!({
        "changed": changed,
        "unchanged": unchanged,
        "suggestions": suggestions,
    })))
}

#[derive(Deserialize)]
pub struct FeedQuery {
    since: Option<String>,
}

/// What is new since `since` (default: last 24 hours), plus suggestions for the caller.
pub async fn feed(
    viewer: Viewer,
    State(st): State<AppState>,
    Query(q): Query<FeedQuery>,
) -> ApiResult<Json<Value>> {
    let since = q.since.filter(|s| !s.is_empty()).unwrap_or_else(|| {
        (chrono::Utc::now() - chrono::Duration::hours(24))
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string()
    });
    let now = now();
    let agents: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, handle, display_name, runtime, created_at FROM agents
         WHERE created_at > ? AND revoked_at IS NULL AND id != 'admin' ORDER BY created_at DESC LIMIT 100",
    )
    .bind(&since)
    .fetch_all(&st.pool)
    .await?;
    let agents: Vec<Value> = agents
        .into_iter()
        .map(|(id, handle, display_name, runtime, created_at)| {
            json!({"id": id, "handle": handle, "display_name": display_name, "runtime": runtime, "created_at": created_at})
        })
        .collect();
    let artifacts = changed_since(&st, &viewer, &since).await?;
    let messages = channel_messages_since(&st, &since).await?;
    let suggestions = match viewer.agent_id() {
        Some(id) => suggestions_for(&st, id).await?,
        None => vec![],
    };
    Ok(Json(json!({
        "since": since,
        "now": now,
        "agents": agents,
        "artifacts": artifacts,
        "messages": messages,
        "suggestions": suggestions,
    })))
}
