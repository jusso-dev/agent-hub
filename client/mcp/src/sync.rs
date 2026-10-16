//! `hub sync <dir>`: collect memories, skills and MCP configs, strip secrets,
//! and send only what changed since the last successful sync.
//!
//! The cache file and hashes match client/hub.sh, so the shell client and
//! this MCP server can sync the same directory without resending everything.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::sanitize::{looks_secret, mcp_items, md_item, Item};

#[derive(Debug, Default)]
pub struct Collected {
    /// (group, item) where group is `memories`, `skills` or `mcps`.
    pub items: Vec<(&'static str, Item)>,
    /// Items dropped because they still look like they hold a secret.
    pub skipped: Vec<String>,
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    v.retain(|p| {
        !p.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".")
            .starts_with('.')
    });
    v.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    v
}

pub fn collect(dir: &Path) -> Result<Collected> {
    let mut c = Collected::default();
    let push = |group: &'static str, item: Item, c: &mut Collected| {
        if looks_secret(&format!("{} {}", item.content, item.summary)) {
            c.skipped.push(format!("{group}/{}", item.name));
        } else {
            c.items.push((group, item));
        }
    };
    for p in sorted_entries(&dir.join("memories")) {
        if p.extension().and_then(|e| e.to_str()) == Some("md") && p.is_file() {
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let text =
                std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
            push("memories", md_item(&text, &name), &mut c);
        }
    }
    for d in sorted_entries(&dir.join("skills")) {
        let f = d.join("SKILL.md");
        if f.is_file() {
            let name = d
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let text =
                std::fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?;
            push("skills", md_item(&text, &name), &mut c);
        }
    }
    let mcp = dir.join("mcp.json");
    if mcp.is_file() {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&mcp)?)
            .with_context(|| format!("could not parse {}", mcp.display()))?;
        for item in mcp_items(&doc)? {
            push("mcps", item, &mut c);
        }
    }
    Ok(c)
}

/// sha256 of the item as compact JSON with sorted keys plus a newline, which
/// is exactly what `jq -cS ... | sha256sum` produces in hub.sh.
pub fn item_hash(item: &Item) -> String {
    let v = json!({"content": item.content, "name": item.name, "summary": item.summary, "tags": item.tags});
    let mut s = serde_json::to_string(&v).expect("serialise item");
    s.push('\n');
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub fn cache_path(cfg: &Config, dir: &Path) -> PathBuf {
    let id = format!("{}\n{}\n{}", cfg.url, cfg.key_tag(), dir.display());
    let h = hex::encode(Sha256::digest(id.as_bytes()));
    cfg.home.join("sync-cache").join(format!("{h}.json"))
}

pub struct Plan {
    pub payload: Value,
    pub all: BTreeMap<String, String>,
    pub sent: usize,
    pub total: usize,
}

pub fn plan(c: &Collected, old: &BTreeMap<String, String>) -> Plan {
    let mut groups: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for g in ["memories", "skills", "mcps"] {
        groups.insert(g, vec![]);
    }
    let mut all = BTreeMap::new();
    let mut sent = 0;
    for (g, item) in &c.items {
        let key = format!("{g}/{}", item.name);
        let h = item_hash(item);
        if old.get(&key) != Some(&h) {
            groups
                .get_mut(g)
                .expect("known group")
                .push(serde_json::to_value(item).expect("item"));
            sent += 1;
        }
        all.insert(key, h);
    }
    let payload =
        json!({"memories": groups["memories"], "skills": groups["skills"], "mcps": groups["mcps"]});
    Plan {
        payload,
        total: all.len(),
        all,
        sent,
    }
}

pub fn load_cache(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_cache(path: &Path, all: &BTreeMap<String, String>) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(all)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_jq() {
        // printf '%s' '{"name":"n","summary":"s","tags":["a"],"content":"c\n"}' | jq -cS . | sha256sum
        let it = Item {
            name: "n".into(),
            summary: "s".into(),
            tags: vec!["a".into()],
            content: "c\n".into(),
        };
        assert_eq!(
            item_hash(&it),
            "417ec389438696f6f0d62af102bf6fb922a1abea1eb292f12456721834d19c31"
        );
    }
}
