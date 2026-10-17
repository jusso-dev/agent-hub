use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use axum::extract::{Path, Query, State};
use axum::Json;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{Agent, AgentRow, Viewer};
use crate::error::{ApiError, ApiResult};
use crate::routes::messages::{store_message, Dest, Stored};
use crate::util::{new_id, norm_tags, now, split_tags};
use crate::AppState;

pub const KINDS: [&str; 4] = ["skill", "knowledge", "memory", "mcp"];

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    owner_agent: String,
    owner: String,
    kind: String,
    name: String,
    summary: String,
    tags: String,
    content: String,
    version: i64,
    visibility: String,
    created_at: String,
    updated_at: String,
    adoption_count: i64,
}

/// Artifact metadata as returned by the API. `content` only on full reads.
#[derive(Clone, Debug, Serialize)]
pub struct ArtifactOut {
    pub id: String,
    pub owner: String,
    pub owner_agent: String,
    pub kind: String,
    pub name: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub version: i64,
    pub visibility: String,
    pub created_at: String,
    pub updated_at: String,
    pub adoption_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl Row {
    fn out(self, with_content: bool) -> ArtifactOut {
        ArtifactOut {
            tags: split_tags(&self.tags),
            content: with_content.then_some(self.content),
            id: self.id,
            owner: self.owner,
            owner_agent: self.owner_agent,
            kind: self.kind,
            name: self.name,
            summary: self.summary,
            version: self.version,
            visibility: self.visibility,
            created_at: self.created_at,
            updated_at: self.updated_at,
            adoption_count: self.adoption_count,
        }
    }
}

const ART_SELECT: &str =
    "SELECT a.id, a.owner_agent, o.handle AS owner, a.kind, a.name, a.summary, a.tags,
    a.content, a.version, a.visibility, a.created_at, a.updated_at,
    (SELECT COUNT(*) FROM adoptions d WHERE d.artifact_id = a.id) AS adoption_count
    FROM artifacts a JOIN agents o ON o.id = a.owner_agent";

/// Tags may be sent as `["a","b"]` or `"a,b"`.
#[derive(Deserialize, Default)]
#[serde(untagged)]
pub enum TagsIn {
    List(Vec<String>),
    Csv(String),
    #[default]
    None,
}

impl TagsIn {
    pub fn norm(&self) -> Vec<String> {
        match self {
            TagsIn::List(v) => norm_tags(v),
            TagsIn::Csv(s) => split_tags(s),
            TagsIn::None => vec![],
        }
    }
}

static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\bsk-[A-Za-z0-9_\-]{16,}",
        r"\bghp_[A-Za-z0-9]{8,}",
        r"\bgho_[A-Za-z0-9]{8,}",
        r"\bxox[abprse]-[A-Za-z0-9\-]{6,}",
        r"\bAKIA[0-9A-Z]{12,}",
        r"-----BEGIN",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

static SECRET_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)"[A-Za-z0-9_\-]*(password|token)"\s*:\s*"([^"]*)""#).unwrap()
});

/// True when an MCP config holds something that looks like a real secret.
pub fn has_secret(content: &str) -> bool {
    if SECRET_PATTERNS.iter().any(|r| r.is_match(content)) {
        return true;
    }
    SECRET_FIELD.captures_iter(content).any(|c| {
        let v = c[2].trim();
        !v.is_empty() && !(v.starts_with("${") && v.ends_with('}'))
    })
}

pub fn secret_error(names: &[String]) -> ApiError {
    let what = if names.is_empty() {
        "Content looks like it contains a secret".to_string()
    } else {
        format!(
            "Content looks like it contains a secret: {}",
            names.join(", ")
        )
    };
    ApiError::unprocessable(what).with_hint(
        "Strip keys, tokens and passwords. Use ${ENV_VAR} placeholders instead, e.g. \"token\": \"${GITHUB_TOKEN}\".",
    )
}

fn valid_name(n: &str) -> bool {
    (1..=100).contains(&n.len())
        && n.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.')
}

/// Memories default to private; everything else defaults to shared.
pub fn default_visibility(kind: &str) -> &'static str {
    if kind == "memory" {
        "private"
    } else {
        "shared"
    }
}

pub struct UpsertInput {
    pub kind: String,
    pub name: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub content: String,
    pub visibility: Option<String>,
}

/// Check an upsert before writing anything.
pub fn validate(input: &UpsertInput) -> ApiResult<()> {
    if !KINDS.contains(&input.kind.as_str()) {
        return Err(ApiError::bad_request(
            "kind must be skill, knowledge, memory or mcp",
        ));
    }
    if !valid_name(&input.name) {
        return Err(ApiError::bad_request(format!(
            "bad name {:?}: use 1-100 chars of a-z, A-Z, 0-9, -, _ or .",
            input.name
        )));
    }
    if input.content.trim().is_empty() {
        return Err(ApiError::bad_request(format!(
            "{} has no content",
            input.name
        )));
    }
    if let Some(v) = &input.visibility {
        if v != "private" && v != "shared" {
            return Err(ApiError::bad_request(
                "visibility must be private or shared",
            ));
        }
    }
    if leaks_secret(input) {
        return Err(secret_error(std::slice::from_ref(&input.name)));
    }
    Ok(())
}

/// MCP configs and anything shared with other agents must not hold secrets.
/// Private memories are only readable by their owner, so they are not scanned.
pub fn leaks_secret(input: &UpsertInput) -> bool {
    let vis = input
        .visibility
        .as_deref()
        .unwrap_or_else(|| default_visibility(&input.kind));
    (input.kind == "mcp" || vis == "shared") && has_secret(&input.content)
}

async fn fetch(st: &AppState, id: &str) -> Result<Option<Row>, sqlx::Error> {
    sqlx::query_as::<_, Row>(&format!("{ART_SELECT} WHERE a.id = ?"))
        .bind(id)
        .fetch_optional(&st.pool)
        .await
}

/// Create or update an artifact. A new version is written when anything changed.
/// Returns the artifact metadata and whether it changed. Shares go out for shared changes.
pub async fn upsert_artifact(
    st: &AppState,
    owner: &AgentRow,
    input: UpsertInput,
) -> ApiResult<(ArtifactOut, bool)> {
    validate(&input)?;
    let visibility = input
        .visibility
        .clone()
        .unwrap_or_else(|| default_visibility(&input.kind).to_string());
    let tags = input.tags.join(",");
    let ts = now();

    let mut tx = st.pool.begin().await?;
    let existing: Option<(String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT id, summary, tags, content, visibility, version FROM artifacts
         WHERE owner_agent = ? AND kind = ? AND name = ?",
    )
    .bind(&owner.id)
    .bind(&input.kind)
    .bind(&input.name)
    .fetch_optional(&mut *tx)
    .await?;

    let (id, version) = match existing {
        Some((id, s, t, c, v, _))
            if s == input.summary && t == tags && c == input.content && v == visibility =>
        {
            tx.rollback().await?;
            let row = fetch(st, &id).await?.expect("artifact exists");
            return Ok((row.out(false), false));
        }
        Some((id, _, _, _, _, ver)) => {
            let version = ver + 1;
            sqlx::query(
                "UPDATE artifacts SET summary = ?, tags = ?, content = ?, visibility = ?, version = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(&input.summary)
            .bind(&tags)
            .bind(&input.content)
            .bind(&visibility)
            .bind(version)
            .bind(&ts)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
            (id, version)
        }
        None => {
            let id = new_id();
            sqlx::query(
                "INSERT INTO artifacts (id, owner_agent, kind, name, summary, tags, content, version, visibility, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)",
            )
            .bind(&id)
            .bind(&owner.id)
            .bind(&input.kind)
            .bind(&input.name)
            .bind(&input.summary)
            .bind(&tags)
            .bind(&input.content)
            .bind(&visibility)
            .bind(&ts)
            .bind(&ts)
            .execute(&mut *tx)
            .await?;
            (id, 1)
        }
    };
    sqlx::query(
        "INSERT INTO artifact_versions (artifact_id, version, content, summary, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(version)
    .bind(&input.content)
    .bind(&input.summary)
    .bind(&ts)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let art = fetch(st, &id).await?.expect("artifact exists").out(false);
    if art.visibility == "shared" {
        notify_share(st, owner, &art).await?;
    }
    Ok((art, true))
}

/// Agent id -> tags it cares about: its interests plus the tags on its own artifacts.
async fn tag_profiles(st: &AppState) -> Result<HashMap<String, HashSet<String>>, sqlx::Error> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    let interests: Vec<(String, String)> = sqlx::query_as(
        "SELECT i.agent_id, i.tag FROM interests i JOIN agents a ON a.id = i.agent_id WHERE a.revoked_at IS NULL",
    )
    .fetch_all(&st.pool)
    .await?;
    for (a, t) in interests {
        map.entry(a).or_default().insert(t);
    }
    let own: Vec<(String, String)> = sqlx::query_as(
        "SELECT x.owner_agent, x.tags FROM artifacts x JOIN agents a ON a.id = x.owner_agent WHERE a.revoked_at IS NULL",
    )
    .fetch_all(&st.pool)
    .await?;
    for (a, tags) in own {
        map.entry(a).or_default().extend(split_tags(&tags));
    }
    Ok(map)
}

/// Proactive sharing: tell interested agents and post to #shared.
async fn notify_share(st: &AppState, owner: &AgentRow, art: &ArtifactOut) -> ApiResult<()> {
    let body = format!(
        "@{} shared {} `{}` v{}: {}. GET /api/artifacts/{} to learn it, POST /api/artifacts/{}/adopt if you use it.",
        owner.handle,
        art.kind,
        art.name,
        art.version,
        art.summary.trim_end_matches('.'),
        art.id,
        art.id
    );
    let tags: HashSet<&String> = art.tags.iter().collect();
    let mut recipients: Vec<String> = tag_profiles(st)
        .await?
        .into_iter()
        .filter(|(aid, t)| aid != &owner.id && t.iter().any(|x| tags.contains(x)))
        .map(|(aid, _)| aid)
        .collect();
    recipients.sort();

    let mut tx = st.pool.begin().await?;
    let mut stored: Vec<Stored> = Vec::new();
    for aid in &recipients {
        stored.push(
            store_message(
                &mut tx,
                &owner.id,
                Dest::Agent(aid),
                None,
                &body,
                "share",
                Some(&art.id),
            )
            .await?,
        );
    }
    stored.push(
        store_message(
            &mut tx,
            &owner.id,
            Dest::Channel("shared"),
            None,
            &body,
            "share",
            Some(&art.id),
        )
        .await?,
    );
    tx.commit().await?;
    for s in &stored {
        s.emit(st);
    }
    st.emit(None, "feed", json!({"type": "artifact", "artifact": art}));
    Ok(())
}

#[derive(Deserialize)]
pub struct UpsertReq {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    tags: TagsIn,
    content: String,
    visibility: Option<String>,
}

pub async fn upsert(
    Agent(me): Agent,
    State(st): State<AppState>,
    Path((kind, name)): Path<(String, String)>,
    Json(req): Json<UpsertReq>,
) -> ApiResult<Json<Value>> {
    let input = UpsertInput {
        kind: kind.to_lowercase(),
        name,
        summary: req.summary.trim().to_string(),
        tags: req.tags.norm(),
        content: req.content,
        visibility: req.visibility,
    };
    let (art, changed) = upsert_artifact(&st, &me, input).await?;
    Ok(Json(json!({ "artifact": art, "changed": changed })))
}

#[derive(Deserialize)]
pub struct ListQuery {
    kind: Option<String>,
    tag: Option<String>,
    q: Option<String>,
    owner: Option<String>,
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Metadata list. Agents see shared artifacts plus their own private ones; admin sees all.
pub async fn list(
    viewer: Viewer,
    State(st): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let mut sql = format!("{ART_SELECT} WHERE 1 = 1");
    let mut binds: Vec<String> = Vec::new();
    if let Some(me) = viewer.agent_id() {
        sql.push_str(" AND (a.visibility = 'shared' OR a.owner_agent = ?)");
        binds.push(me.to_string());
    }
    if let Some(k) = q.kind.filter(|s| !s.is_empty()) {
        sql.push_str(" AND a.kind = ?");
        binds.push(k.to_lowercase());
    }
    if let Some(t) = q.tag.filter(|s| !s.is_empty()) {
        sql.push_str(" AND (',' || a.tags || ',') LIKE ? ESCAPE '\\'");
        binds.push(format!("%,{},%", like_escape(&t.trim().to_lowercase())));
    }
    if let Some(o) = q.owner.filter(|s| !s.is_empty()) {
        sql.push_str(" AND o.handle = ?");
        binds.push(o.trim_start_matches('@').to_lowercase());
    }
    if let Some(text) = q.q.filter(|s| !s.is_empty()) {
        sql.push_str(
            " AND (a.name LIKE ? ESCAPE '\\' OR a.summary LIKE ? ESCAPE '\\' OR a.tags LIKE ? ESCAPE '\\' OR a.content LIKE ? ESCAPE '\\')",
        );
        let pat = format!("%{}%", like_escape(&text));
        binds.extend(std::iter::repeat_n(pat, 4));
    }
    sql.push_str(" ORDER BY a.updated_at DESC LIMIT 200");
    let mut query = sqlx::query_as::<_, Row>(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let rows = query.fetch_all(&st.pool).await?;
    let out: Vec<ArtifactOut> = rows.into_iter().map(|r| r.out(false)).collect();
    Ok(Json(json!({ "artifacts": out })))
}

fn can_see(viewer: &Viewer, row: &Row) -> bool {
    match viewer.agent_id() {
        None => true,
        Some(me) => row.visibility == "shared" || row.owner_agent == me,
    }
}

#[derive(Serialize, sqlx::FromRow)]
struct VersionOut {
    version: i64,
    summary: String,
    content: String,
    created_at: String,
}

#[derive(Serialize, sqlx::FromRow)]
struct AdoptionOut {
    agent_id: String,
    handle: String,
    version: i64,
    adopted_at: String,
}

#[derive(Deserialize)]
pub struct GetQuery {
    version: Option<i64>,
}

/// Full artifact with version history (each with content) and who adopted it.
/// With `?version=N`: the current artifact plus just that version.
pub async fn get_one(
    viewer: Viewer,
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<GetQuery>,
) -> ApiResult<Json<Value>> {
    let row = fetch(&st, &id)
        .await?
        .filter(|r| can_see(&viewer, r))
        .ok_or_else(|| ApiError::not_found("no such artifact"))?;
    if let Some(n) = q.version {
        let v: VersionOut = sqlx::query_as(
            "SELECT version, summary, content, created_at FROM artifact_versions WHERE artifact_id = ? AND version = ?",
        )
        .bind(&id)
        .bind(n)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("no such version"))?;
        return Ok(Json(json!({ "artifact": row.out(true), "version": v })));
    }
    let versions: Vec<VersionOut> = sqlx::query_as(
        "SELECT version, summary, content, created_at FROM artifact_versions WHERE artifact_id = ? ORDER BY version DESC",
    )
    .bind(&id)
    .fetch_all(&st.pool)
    .await?;
    let adoptions: Vec<AdoptionOut> = sqlx::query_as(
        "SELECT d.agent_id, a.handle, d.version, d.adopted_at FROM adoptions d JOIN agents a ON a.id = d.agent_id
         WHERE d.artifact_id = ? ORDER BY d.adopted_at DESC",
    )
    .bind(&id)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(json!({
        "artifact": row.out(true),
        "versions": versions,
        "adoptions": adoptions,
    })))
}

/// Record that I use this artifact (current version) and tell its owner.
pub async fn adopt(
    Agent(me): Agent,
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let row = fetch(&st, &id)
        .await?
        .filter(|r| r.visibility == "shared" || r.owner_agent == me.id)
        .ok_or_else(|| ApiError::not_found("no such artifact"))?;
    if row.owner_agent == me.id {
        return Err(ApiError::bad_request("you own this artifact"));
    }
    let ts = now();
    let mut tx = st.pool.begin().await?;
    let prev: Option<(i64,)> =
        sqlx::query_as("SELECT version FROM adoptions WHERE agent_id = ? AND artifact_id = ?")
            .bind(&me.id)
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
    sqlx::query(
        "INSERT INTO adoptions (agent_id, artifact_id, version, adopted_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (agent_id, artifact_id) DO UPDATE SET version = excluded.version, adopted_at = excluded.adopted_at",
    )
    .bind(&me.id)
    .bind(&id)
    .bind(row.version)
    .bind(&ts)
    .execute(&mut *tx)
    .await?;
    let mut stored = None;
    if prev.map(|p| p.0) != Some(row.version) {
        let body = format!(
            "@{} adopted your {} `{}` v{}.",
            me.handle, row.kind, row.name, row.version
        );
        stored = Some(
            store_message(
                &mut tx,
                &me.id,
                Dest::Agent(&row.owner_agent),
                None,
                &body,
                "system",
                Some(&id),
            )
            .await?,
        );
    }
    tx.commit().await?;
    if let Some(s) = stored {
        s.emit(&st);
    }
    Ok(Json(json!({
        "adoption": {"artifact_id": id, "agent_id": me.id, "handle": me.handle, "version": row.version, "adopted_at": ts}
    })))
}

/// Shared artifacts I have not adopted whose tags overlap mine, newest first, max 10.
pub async fn suggestions_for(st: &AppState, agent_id: &str) -> ApiResult<Vec<Value>> {
    let mine = tag_profiles(st).await?.remove(agent_id).unwrap_or_default();
    if mine.is_empty() {
        return Ok(vec![]);
    }
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "{ART_SELECT} WHERE a.visibility = 'shared' AND a.owner_agent != ? AND o.revoked_at IS NULL
         AND NOT EXISTS (SELECT 1 FROM adoptions d WHERE d.artifact_id = a.id AND d.agent_id = ?)
         ORDER BY a.updated_at DESC"
    ))
    .bind(agent_id)
    .bind(agent_id)
    .fetch_all(&st.pool)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        let art = r.out(false);
        let matched: Vec<&String> = art.tags.iter().filter(|t| mine.contains(*t)).collect();
        if matched.is_empty() {
            continue;
        }
        let mut v = json!(art);
        v["matched_tags"] = json!(matched);
        out.push(v);
        if out.len() == 10 {
            break;
        }
    }
    Ok(out)
}

/// Shared artifacts changed since a timestamp, plus my own.
pub async fn changed_since(
    st: &AppState,
    viewer: &Viewer,
    since: &str,
) -> Result<Vec<ArtifactOut>, sqlx::Error> {
    let me = viewer.agent_id().unwrap_or("");
    let admin = viewer.agent_id().is_none();
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "{ART_SELECT} WHERE a.updated_at > ? AND (a.visibility = 'shared' OR a.owner_agent = ? OR ?)
         ORDER BY a.updated_at DESC LIMIT 100"
    ))
    .bind(since)
    .bind(me)
    .bind(admin)
    .fetch_all(&st.pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.out(false)).collect())
}

#[cfg(test)]
mod tests {
    use super::has_secret;

    #[test]
    fn secrets() {
        assert!(has_secret(r#"{"env":{"KEY":"sk-abcdefghijklmnop"}}"#));
        assert!(has_secret("ghp_abcdefghijklmnopqrstuvwxyz0123456789"));
        assert!(has_secret("gho_abcdefghijklmnop"));
        assert!(has_secret("xoxb-1234567890-abcdef"));
        assert!(has_secret("AKIAABCDEFGHIJKLMNOP"));
        assert!(has_secret("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(has_secret(r#"{"password": "hunter2"}"#));
        assert!(has_secret(r#"{"GITHUB_TOKEN": "abc"}"#));
        assert!(!has_secret(r#"{"token": "${GITHUB_TOKEN}"}"#));
        assert!(!has_secret(r#"{"password": ""}"#));
        assert!(!has_secret(
            r#"{"command": "npx", "args": ["task-runner-thing"]}"#
        ));
    }

    #[test]
    fn sk_pattern_needs_key_shape() {
        assert!(has_secret("OPENAI_API_KEY=sk-proj-abcdefghijklmnop1234"));
        assert!(has_secret("sk-abcdefghijklmnop"));
        // Too short to be a key.
        assert!(!has_secret("sk-abcdefghijklmno"));
        // Words ending in "sk" are not keys.
        assert!(!has_secret("task-abcdefghijklmnopqrstuvwxyz"));
        assert!(!has_secret("disk-0123456789abcdefghijklmn"));
        assert!(!has_secret(
            "run the risk-assessment-for-everything-here step"
        ));
    }
}
