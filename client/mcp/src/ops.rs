//! The hub operations behind each MCP tool. Kept apart from the MCP layer so
//! they can be tested directly.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::client::{as_list, seg, HubClient};
use crate::sanitize::{looks_secret, norm_tags_str, share_item};
use crate::sync;

pub const KINDS: [&str; 4] = ["skill", "knowledge", "memory", "mcp"];

pub async fn inbox(c: &HubClient, unread_only: bool, limit: u32, mark_read: bool) -> Result<Value> {
    let res = c
        .get(
            "/inbox",
            &[
                ("unread", unread_only.to_string()),
                ("limit", limit.to_string()),
            ],
        )
        .await?;
    if mark_read {
        let ids: Vec<Value> = as_list(&res)
            .into_iter()
            .filter(|m| m.get("read_at").is_none_or(Value::is_null))
            .filter_map(|m| m.get("id").cloned())
            .collect();
        if !ids.is_empty() {
            c.post("/inbox/read", &json!({"ids": ids})).await?;
        }
    }
    Ok(res)
}

pub async fn send(
    c: &HubClient,
    to: &str,
    body: &str,
    thread_id: Option<&str>,
    kind: Option<&str>,
) -> Result<Value> {
    if !(to.starts_with('@') || to.starts_with('#')) {
        bail!("`to` must be @handle or #channel");
    }
    if body.trim().is_empty() {
        bail!("`body` is empty");
    }
    let mut b = json!({"to": to, "body": body, "kind": kind.unwrap_or("chat")});
    if let Some(t) = thread_id {
        b["thread_id"] = json!(t);
    }
    c.post("/messages", &b).await
}

#[allow(clippy::too_many_arguments)]
pub async fn share(
    c: &HubClient,
    kind: &str,
    name: &str,
    content: Option<&str>,
    path: Option<&str>,
    summary: Option<&str>,
    tags: &[String],
    private: bool,
) -> Result<Value> {
    if !KINDS.contains(&kind) {
        bail!("kind must be one of skill, knowledge, memory, mcp");
    }
    let text = match (content, path) {
        (Some(t), _) => t.to_string(),
        (None, Some(p)) => std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?,
        (None, None) => bail!("give either `content` or `path`"),
    };
    let item = share_item(kind, name, &text)?;
    if looks_secret(&item.content) {
        bail!("content still looks like it holds a secret after stripping. Replace it with a ${{ENV_VAR}} placeholder.");
    }
    let mut all_tags = item.tags.clone();
    all_tags.extend(tags.iter().cloned());
    let body = json!({
        "summary": summary.filter(|s| !s.is_empty()).map_or(item.summary.clone(), str::to_string),
        "tags": norm_tags_str(&all_tags),
        "content": item.content,
        "visibility": if private { "private" } else { "shared" },
    });
    c.put(&format!("/artifacts/{}/{}", seg(kind), seg(name)), &body)
        .await
}

pub async fn search(
    c: &HubClient,
    q: Option<&str>,
    kind: Option<&str>,
    tag: Option<&str>,
    owner: Option<&str>,
) -> Result<Value> {
    let mut query = vec![];
    for (k, v) in [
        ("q", q),
        ("kind", kind),
        ("tag", tag),
        ("owner", owner.map(|o| o.trim_start_matches('@'))),
    ] {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            query.push((k, v.to_string()));
        }
    }
    c.get("/artifacts", &query).await
}

pub async fn get(c: &HubClient, id: &str, version: Option<u32>) -> Result<Value> {
    let q: Vec<(&str, String)> = match version {
        Some(v) => vec![("version", v.to_string())],
        None => vec![],
    };
    c.get(&format!("/artifacts/{}", seg(id)), &q).await
}

pub async fn adopt(c: &HubClient, id: &str) -> Result<Value> {
    c.post(&format!("/artifacts/{}/adopt", seg(id)), &json!({}))
        .await
}

pub async fn feed(c: &HubClient, since: Option<&str>, all: bool) -> Result<Value> {
    let since_file = c.cfg.home.join("feed-since");
    let stored = std::fs::read_to_string(&since_file)
        .ok()
        .map(|s| s.trim().to_string());
    let since = since
        .map(str::to_string)
        .or(if all { None } else { stored })
        .filter(|s| !s.is_empty());
    let q: Vec<(&str, String)> = since.iter().map(|s| ("since", s.clone())).collect();
    let res = c.get("/feed", &q).await?;
    if let Some(now) = res.get("now").and_then(Value::as_str) {
        std::fs::create_dir_all(&c.cfg.home).ok();
        std::fs::write(&since_file, format!("{now}\n")).ok();
    }
    Ok(res)
}

pub async fn sync_dir(c: &HubClient, dir: &Path, dry_run: bool, force: bool) -> Result<Value> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("no such directory {}", dir.display()))?;
    let collected = sync::collect(&dir)?;
    let cache = sync::cache_path(&c.cfg, &dir);
    let old = if force {
        Default::default()
    } else {
        sync::load_cache(&cache)
    };
    let plan = sync::plan(&collected, &old);
    if dry_run {
        return Ok(json!({
            "dry_run": true,
            "sent": plan.sent,
            "total": plan.total,
            "skipped": collected.skipped,
            "payload": plan.payload,
        }));
    }
    let mut res = c.post("/sync", &plan.payload).await?;
    sync::save_cache(&cache, &plan.all)?;
    if let Value::Object(m) = &mut res {
        m.insert("sent".into(), json!(plan.sent));
        m.insert("total".into(), json!(plan.total));
        m.insert("skipped".into(), json!(collected.skipped));
    }
    Ok(res)
}
