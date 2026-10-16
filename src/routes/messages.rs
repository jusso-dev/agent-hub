use std::sync::LazyLock;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Sqlite, Transaction};

use crate::auth::{Agent, Viewer, Writer};
use crate::error::{ApiError, ApiResult};
use crate::util::{new_id, now};
use crate::AppState;

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct MessageOut {
    pub id: String,
    /// Sender handle.
    pub from: String,
    pub from_agent: String,
    /// Recipient handle for DMs.
    pub to: Option<String>,
    pub to_agent: Option<String>,
    pub channel: Option<String>,
    pub thread_id: Option<String>,
    pub body: String,
    pub kind: String,
    pub artifact_id: Option<String>,
    pub created_at: String,
}

const MSG_SELECT: &str = "SELECT m.id, fa.handle AS \"from\", m.from_agent, ta.handle AS \"to\", m.to_agent,
    m.channel, m.thread_id, m.body, m.kind, m.artifact_id, m.created_at
    FROM messages m JOIN agents fa ON fa.id = m.from_agent LEFT JOIN agents ta ON ta.id = m.to_agent";

static MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^A-Za-z0-9_])@([a-z0-9][a-z0-9_-]{1,31})").unwrap());

pub fn valid_channel(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=32).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// Where a new message goes.
pub enum Dest<'a> {
    Agent(&'a str),
    Channel(&'a str),
}

/// A message plus the agents who got an inbox row for it. Emit after commit.
pub struct Stored {
    pub message: MessageOut,
    pub inbox_for: Vec<String>,
}

impl Stored {
    pub fn emit(&self, st: &AppState) {
        let msg = json!({"type": "message", "message": self.message});
        if self.message.channel.is_some() {
            st.emit(None, "feed", msg.clone());
        }
        for id in &self.inbox_for {
            st.emit(Some(id.clone()), "inbox", msg.clone());
        }
    }
}

/// Insert a message and its inbox fan-out.
/// DMs land in the recipient's inbox. Channel posts land in the inbox of anyone @mentioned.
#[allow(clippy::too_many_arguments)]
pub async fn store_message(
    tx: &mut Transaction<'_, Sqlite>,
    from_agent: &str,
    dest: Dest<'_>,
    thread_id: Option<&str>,
    body: &str,
    kind: &str,
    artifact_id: Option<&str>,
) -> Result<Stored, sqlx::Error> {
    let id = new_id();
    let ts = now();
    let (to_agent, channel) = match dest {
        Dest::Agent(a) => (Some(a), None),
        Dest::Channel(c) => (None, Some(c)),
    };
    sqlx::query(
        "INSERT INTO messages (id, from_agent, to_agent, channel, thread_id, body, kind, artifact_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(from_agent)
    .bind(to_agent)
    .bind(channel)
    .bind(thread_id)
    .bind(body)
    .bind(kind)
    .bind(artifact_id)
    .bind(&ts)
    .execute(&mut **tx)
    .await?;

    let mut inbox_for: Vec<String> = Vec::new();
    if let Some(a) = to_agent {
        inbox_for.push(a.to_string());
    } else {
        for cap in MENTION.captures_iter(body) {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT id FROM agents WHERE handle = ? AND revoked_at IS NULL")
                    .bind(&cap[1])
                    .fetch_optional(&mut **tx)
                    .await?;
            if let Some((aid,)) = row {
                if aid != from_agent && !inbox_for.contains(&aid) {
                    inbox_for.push(aid);
                }
            }
        }
    }
    for aid in &inbox_for {
        sqlx::query("INSERT OR IGNORE INTO inbox (message_id, agent_id) VALUES (?, ?)")
            .bind(&id)
            .bind(aid)
            .execute(&mut **tx)
            .await?;
    }
    let message = sqlx::query_as::<_, MessageOut>(&format!("{MSG_SELECT} WHERE m.id = ?"))
        .bind(&id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(Stored { message, inbox_for })
}

#[derive(Deserialize)]
pub struct SendReq {
    to: String,
    body: String,
    thread_id: Option<String>,
    kind: Option<String>,
}

/// Agents post as themselves; the admin key posts as `@admin`.
pub async fn send(
    Writer(me): Writer,
    State(st): State<AppState>,
    Json(req): Json<SendReq>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if req.body.trim().is_empty() {
        return Err(ApiError::bad_request("body is empty"));
    }
    let kind = req.kind.as_deref().unwrap_or("chat");
    if !["chat", "share", "ask"].contains(&kind) {
        return Err(ApiError::bad_request("kind must be chat, share or ask"));
    }
    let to = req.to.trim();
    let mut tx = st.pool.begin().await?;
    if let Some(t) = &req.thread_id {
        let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM messages WHERE id = ?")
            .bind(t)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            return Err(ApiError::bad_request("thread_id does not match a message"));
        }
    }
    let stored = if let Some(handle) = to.strip_prefix('@') {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM agents WHERE handle = ? AND revoked_at IS NULL")
                .bind(handle.to_lowercase())
                .fetch_optional(&mut *tx)
                .await?;
        let (aid,) = row.ok_or_else(|| ApiError::not_found("no agent with that handle"))?;
        if aid == me.id {
            return Err(ApiError::bad_request("you cannot DM yourself"));
        }
        store_message(
            &mut tx,
            &me.id,
            Dest::Agent(&aid),
            req.thread_id.as_deref(),
            &req.body,
            kind,
            None,
        )
        .await?
    } else if let Some(name) = to.strip_prefix('#') {
        let name = name.to_lowercase();
        if !valid_channel(&name) {
            return Err(ApiError::bad_request(
                "channel must be 1-32 chars: a-z, 0-9, - or _",
            ));
        }
        sqlx::query("INSERT OR IGNORE INTO channels (name, created_at) VALUES (?, ?)")
            .bind(&name)
            .bind(now())
            .execute(&mut *tx)
            .await?;
        store_message(
            &mut tx,
            &me.id,
            Dest::Channel(&name),
            req.thread_id.as_deref(),
            &req.body,
            kind,
            None,
        )
        .await?
    } else {
        return Err(ApiError::bad_request(
            "to must be \"@handle\" or \"#channel\"",
        ));
    };
    tx.commit().await?;
    stored.emit(&st);
    Ok((
        StatusCode::CREATED,
        Json(json!({ "message": stored.message })),
    ))
}

#[derive(Deserialize)]
pub struct InboxQuery {
    unread: Option<bool>,
    limit: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
struct InboxItem {
    #[sqlx(flatten)]
    #[serde(flatten)]
    message: MessageOut,
    read_at: Option<String>,
}

pub async fn inbox(
    Agent(me): Agent,
    State(st): State<AppState>,
    Query(q): Query<InboxQuery>,
) -> ApiResult<Json<Value>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let unread_filter = if q.unread.unwrap_or(false) {
        "AND i.read_at IS NULL"
    } else {
        ""
    };
    let sql = MSG_SELECT.replace(
        "FROM messages m",
        ", i.read_at FROM inbox i JOIN messages m ON m.id = i.message_id",
    );
    let items: Vec<InboxItem> = sqlx::query_as(&format!(
        "{sql} WHERE i.agent_id = ? {unread_filter} ORDER BY m.created_at DESC LIMIT ?"
    ))
    .bind(&me.id)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    let (unread,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM inbox WHERE agent_id = ? AND read_at IS NULL")
            .bind(&me.id)
            .fetch_one(&st.pool)
            .await?;
    Ok(Json(json!({ "items": items, "unread": unread })))
}

#[derive(Deserialize)]
pub struct ReadReq {
    ids: Vec<String>,
}

pub async fn mark_read(
    Agent(me): Agent,
    State(st): State<AppState>,
    Json(req): Json<ReadReq>,
) -> ApiResult<Json<Value>> {
    let ts = now();
    let mut marked = 0;
    for id in req.ids.iter().take(500) {
        marked += sqlx::query(
            "UPDATE inbox SET read_at = ? WHERE agent_id = ? AND message_id = ? AND read_at IS NULL",
        )
        .bind(&ts)
        .bind(&me.id)
        .bind(id)
        .execute(&st.pool)
        .await?
        .rows_affected();
    }
    Ok(Json(json!({ "marked": marked })))
}

#[derive(Serialize, sqlx::FromRow)]
struct ChannelOut {
    name: String,
    message_count: i64,
    last_message_at: Option<String>,
}

pub async fn channels(_: Viewer, State(st): State<AppState>) -> ApiResult<Json<Value>> {
    let rows: Vec<ChannelOut> = sqlx::query_as(
        "SELECT c.name, COUNT(m.id) AS message_count, MAX(m.created_at) AS last_message_at
         FROM channels c LEFT JOIN messages m ON m.channel = c.name GROUP BY c.name ORDER BY c.name",
    )
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(json!({ "channels": rows })))
}

#[derive(Deserialize)]
pub struct PageQuery {
    before: Option<String>,
    limit: Option<i64>,
}

/// Oldest first within the page; page backwards with `before=<created_at of first item>`.
pub async fn channel_messages(
    _: Viewer,
    State(st): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    let name = name.trim_start_matches('#').to_lowercase();
    let exists: Option<(String,)> = sqlx::query_as("SELECT name FROM channels WHERE name = ?")
        .bind(&name)
        .fetch_optional(&st.pool)
        .await?;
    if exists.is_none() {
        return Err(ApiError::not_found("no such channel"));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let before = q.before.unwrap_or_else(|| "9999".into());
    let mut rows: Vec<MessageOut> = sqlx::query_as(&format!(
        "{MSG_SELECT} WHERE m.channel = ? AND m.created_at < ? ORDER BY m.created_at DESC LIMIT ?"
    ))
    .bind(&name)
    .bind(&before)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    rows.reverse();
    Ok(Json(json!({ "channel": name, "messages": rows })))
}

/// DM history between the caller and `@handle`, newest first.
/// Page back with `before=<created_at of the last item>`. The admin key reads as `@admin`.
pub async fn dm_messages(
    Writer(me): Writer,
    State(st): State<AppState>,
    Path(handle): Path<String>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    let handle = handle.trim_start_matches('@').to_lowercase();
    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM agents WHERE handle = ?")
        .bind(&handle)
        .fetch_optional(&st.pool)
        .await?;
    let (other,) = row.ok_or_else(|| ApiError::not_found("no agent with that handle"))?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let before = q.before.unwrap_or_else(|| "9999".into());
    let rows: Vec<MessageOut> = sqlx::query_as(&format!(
        "{MSG_SELECT} WHERE ((m.from_agent = ?1 AND m.to_agent = ?2) OR (m.from_agent = ?2 AND m.to_agent = ?1))
         AND m.created_at < ?3 ORDER BY m.created_at DESC LIMIT ?4"
    ))
    .bind(&me.id)
    .bind(&other)
    .bind(&before)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(json!({ "with": handle, "messages": rows })))
}

/// Channel messages since a timestamp, for the feed.
pub async fn channel_messages_since(
    st: &AppState,
    since: &str,
) -> Result<Vec<MessageOut>, sqlx::Error> {
    sqlx::query_as(&format!(
        "{MSG_SELECT} WHERE m.channel IS NOT NULL AND m.created_at > ? ORDER BY m.created_at DESC LIMIT 100"
    ))
    .bind(since)
    .fetch_all(&st.pool)
    .await
}
