//! Secret stripping and markdown front matter parsing.
//!
//! These rules mirror the jq library in `client/hub.sh`. Both are checked
//! against `client/test/fixtures`, so a change here needs the same change there.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::{Captures, NoExpand, Regex};
use serde::Serialize;
use serde_json::{Map, Value};

/// One item in a `POST /api/sync` payload.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Item {
    pub name: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub content: String,
}

fn re(s: &str) -> Regex {
    Regex::new(s).expect("static regex")
}

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| re(r"^\$\{[A-Za-z0-9_]+\}$"));
static SECRET_KEY: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)(token|secret|password|passwd|pwd|api[-_]?key|apikey|auth|credential|private[-_]?key|access[-_]?key|cookie|session)",
    )
});
static LOOKS_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"\bsk-[A-Za-z0-9_-]{16,}|ghp_[A-Za-z0-9]|gho_[A-Za-z0-9]|\bxox[abposr]-|AKIA[0-9A-Z]{16}|-----BEGIN",
    )
});

/// Fixed replacements, applied in order.
static FIXED: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            re(r"-----BEGIN [A-Z0-9 ]+-----[\s\S]*?-----END [A-Z0-9 ]+-----"),
            "${PRIVATE_KEY}",
        ),
        (re(r"\bsk-ant-[A-Za-z0-9_-]{10,}"), "${ANTHROPIC_API_KEY}"),
        (re(r"\bsk-[A-Za-z0-9_-]{16,}"), "${OPENAI_API_KEY}"),
        (re(r"\bgithub_pat_[A-Za-z0-9_]{20,}"), "${GITHUB_TOKEN}"),
        (re(r"\bgh[pousr]_[A-Za-z0-9]{20,}"), "${GITHUB_TOKEN}"),
        (re(r"\bxox[abposr]-[A-Za-z0-9-]{10,}"), "${SLACK_TOKEN}"),
        (
            re(r"https://hooks\.slack\.com/services/[A-Za-z0-9/_-]+"),
            "${SLACK_WEBHOOK_URL}",
        ),
        (re(r"\bAKIA[0-9A-Z]{16}\b"), "${AWS_ACCESS_KEY_ID}"),
        (re(r"\bah_[A-Za-z0-9_-]{20,}"), "${HUB_KEY}"),
    ]
});
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?<a>\b[Bb]earer\s+)[A-Za-z0-9._~+/=-]{16,}"));
static URL_PASS: LazyLock<Regex> = LazyLock::new(|| re(r"(?<a>://[^/\s:@]+:)[^@\s/$][^@\s/]*@"));
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"(?<a>\b(?<k>(?i:password|passwd|secret|api[_-]?key|access[_-]?token|auth[_-]?token|client[_-]?secret))["']?\s*[:=]\s*)["']?[^\s"'$][^\s"']{7,}["']?"#,
    )
});

/// Upper-case a name into an environment variable name.
pub fn env_name(s: &str) -> String {
    let up: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    // Collapse runs of `_` and trim them from both ends, as the jq version does.
    let mut out = String::with_capacity(up.len());
    for c in up.chars() {
        if c == '_' && out.ends_with('_') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches('_').to_string()
}

fn placeholder(server: &str, key: &str) -> String {
    format!("${{{}}}", env_name(&format!("{server}_{key}")))
}

pub fn is_placeholder(s: &str) -> bool {
    PLACEHOLDER.is_match(s)
}

pub fn is_secret_key(k: &str) -> bool {
    SECRET_KEY.is_match(k)
}

/// True if text still looks like it carries a credential. Used as a last check
/// before upload, close to the server's own 422 rule.
pub fn looks_secret(s: &str) -> bool {
    LOOKS_SECRET.is_match(s)
}

/// Replace well-known credential shapes in free text with `${ENV}` placeholders.
pub fn redact_text(s: &str) -> String {
    let mut out = s.to_string();
    for (r, rep) in FIXED.iter() {
        out = r.replace_all(&out, NoExpand(rep)).into_owned();
    }
    out = BEARER
        .replace_all(&out, |c: &Captures| format!("{}${{BEARER_TOKEN}}", &c["a"]))
        .into_owned();
    out = URL_PASS
        .replace_all(&out, |c: &Captures| {
            format!("{}${{URL_PASSWORD}}@", &c["a"])
        })
        .into_owned();
    out = KEY_VALUE
        .replace_all(&out, |c: &Captures| {
            format!("{}${{{}}}", &c["a"], env_name(&c["k"]))
        })
        .into_owned();
    out
}

static FLAG_EQ: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(?<f>--?(?<n>[A-Za-z0-9_-]+)=)(?<v>[\s\S]*)$"));
static FLAG: LazyLock<Regex> = LazyLock::new(|| re(r"^--?[A-Za-z0-9_-]+$"));
static AUTH_SCHEME: LazyLock<Regex> = LazyLock::new(|| re(r"^(?<s>(?i:bearer|basic|token)\s+)\S"));

/// Strip secrets from one MCP server config. `parent` is the key of the
/// enclosing object, `key` is this value's own key.
pub fn sanitize_json(v: &Value, server: &str, parent: Option<&str>, key: Option<&str>) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, val) in m {
                out.insert(k.clone(), sanitize_json(val, server, key, Some(k)));
            }
            Value::Object(out)
        }
        Value::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for (i, el) in a.iter().enumerate() {
                let Value::String(s) = el else {
                    out.push(sanitize_json(el, server, key, None));
                    continue;
                };
                let new = if s.is_empty() || is_placeholder(s) {
                    s.clone()
                } else if let Some(c) = FLAG_EQ.captures(s) {
                    let v = &c["v"];
                    if is_secret_key(&c["n"]) && !v.is_empty() && !is_placeholder(v) {
                        format!("{}{}", &c["f"], placeholder(server, &c["n"]))
                    } else {
                        redact_text(s)
                    }
                } else if let Some(prev) =
                    i.checked_sub(1).and_then(|j| a[j].as_str()).filter(|p| {
                        FLAG.is_match(p)
                            && is_secret_key(p.trim_start_matches('-'))
                            && !s.starts_with('-')
                    })
                {
                    placeholder(server, prev.trim_start_matches('-'))
                } else {
                    redact_text(s)
                };
                out.push(Value::String(new));
            }
            Value::Array(out)
        }
        Value::String(s) => {
            if s.is_empty() || is_placeholder(s) {
                return v.clone();
            }
            let k = key.unwrap_or("");
            if parent == Some("env") && (is_secret_key(k) || redact_text(s) != *s) {
                return Value::String(format!("${{{}}}", env_name(k)));
            }
            if is_secret_key(k) {
                if let Some(c) = AUTH_SCHEME.captures(s) {
                    return Value::String(format!("{}{}", &c["s"], placeholder(server, k)));
                }
                return Value::String(placeholder(server, k));
            }
            Value::String(redact_text(s))
        }
        _ => v.clone(),
    }
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    let b = t.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// Normalise tags from a JSON value: array of anything, or a comma string,
/// optionally wrapped in `[...]`. Lower-cased, de-duplicated, sorted.
pub fn norm_tags(v: Option<&Value>) -> Vec<String> {
    let raw: Vec<String> = match v {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                o => o.to_string(),
            })
            .collect(),
        Some(Value::String(s)) => {
            let t = s.trim_start();
            let t = t.strip_prefix('[').unwrap_or(t);
            let t = t.trim_end();
            let t = t.strip_suffix(']').unwrap_or(t);
            t.split(',').map(str::to_string).collect()
        }
        Some(o) => vec![o.to_string()],
    };
    raw.iter()
        .map(|t| unquote(t).to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn norm_tags_str(tags: &[String]) -> Vec<String> {
    norm_tags(Some(&Value::Array(
        tags.iter().cloned().map(Value::String).collect(),
    )))
}

static FM: LazyLock<Regex> =
    LazyLock::new(|| re(r"^---\r?\n(?<fm>[\s\S]*?)\r?\n---[ \t]*(\r?\n|$)(?<rest>[\s\S]*)$"));
static FM_KEY: LazyLock<Regex> = LazyLock::new(|| re(r"^(?<k>[A-Za-z_][A-Za-z0-9_-]*):(?<v>.*)$"));
static FM_LIST: LazyLock<Regex> = LazyLock::new(|| re(r"^\s+-\s+"));
static FM_CONT: LazyLock<Regex> = LazyLock::new(|| re(r"^\s+\S"));
static FM_BLOCK: LazyLock<Regex> = LazyLock::new(|| re(r"^[|>][-+]?$"));

/// Split YAML front matter from markdown. Returns (front matter, body).
pub fn split_front_matter(s: &str) -> (&str, &str) {
    if s.starts_with("---\n") || s.starts_with("---\r\n") {
        if let Some(c) = FM.captures(s) {
            let fm = c.name("fm").map_or("", |m| m.as_str());
            let rest = c.name("rest").map_or("", |m| m.as_str());
            return (fm, rest);
        }
    }
    ("", s)
}

/// Parse the small YAML subset agents use in SKILL.md front matter.
pub fn front_matter_fields(fm: &str) -> Map<String, Value> {
    let mut out = Map::new();
    let mut cur: Option<String> = None;
    for line in fm.split('\n') {
        let l = line.strip_suffix('\r').unwrap_or(line);
        if let Some(c) = FM_KEY.captures(l) {
            let k = c["k"].to_string();
            let v = c["v"].trim();
            let val = if FM_BLOCK.is_match(v) {
                String::new()
            } else {
                unquote(v)
            };
            out.insert(k.clone(), Value::String(val));
            cur = Some(k);
        } else if let (Some(k), true) = (cur.as_ref(), FM_LIST.is_match(l)) {
            let item = unquote(&FM_LIST.replace(l, ""));
            let mut list = match out.get(k) {
                Some(Value::Array(a)) => a.clone(),
                _ => vec![],
            };
            list.push(Value::String(item));
            out.insert(k.clone(), Value::Array(list));
        } else if let (Some(k), true) = (cur.as_ref(), FM_CONT.is_match(l)) {
            if let Some(Value::String(prev)) = out.get(k) {
                let joined = [prev.as_str(), l.trim()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" ");
                out.insert(k.clone(), Value::String(joined));
            }
        } else if !l.starts_with(|c: char| c.is_whitespace()) {
            cur = None;
        }
    }
    out
}

fn first_line(s: &str) -> String {
    s.split('\n')
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("---"))
        .unwrap_or("")
        .to_string()
}

fn clip(s: &str) -> String {
    if s.chars().count() > 200 {
        let mut c: String = s.chars().take(197).collect();
        c.push_str("...");
        c
    } else {
        s.to_string()
    }
}

fn str_field<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

/// Build a sync item from markdown text. `default_name` is used when the
/// front matter has no `name`.
pub fn md_item(text: &str, default_name: &str) -> Item {
    let (fm, rest) = split_front_matter(text);
    let f = front_matter_fields(fm);
    let name = match str_field(&f, "name") {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => default_name.to_string(),
    };
    let desc = str_field(&f, "description")
        .or_else(|| str_field(&f, "summary"))
        .unwrap_or("");
    let summary = if desc.is_empty() {
        first_line(rest)
    } else {
        desc.to_string()
    };
    Item {
        name,
        summary: clip(&redact_text(&summary)),
        tags: norm_tags(f.get("tags")),
        content: redact_text(text),
    }
}

/// Build one sync item per server in a parsed `mcp.json`.
pub fn mcp_items(doc: &Value) -> anyhow::Result<Vec<Item>> {
    let servers = ["mcpServers", "servers", "mcp_servers"]
        .iter()
        .find_map(|k| doc.get(*k))
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("mcp.json: expected an mcpServers object"))?;
    let mut out = vec![];
    for (name, cfg) in servers {
        let meta = cfg.get("x-hub").cloned().unwrap_or(Value::Null);
        let mut cfg = cfg.clone();
        if let Some(m) = cfg.as_object_mut() {
            m.shift_remove("x-hub");
        }
        let cfg = sanitize_json(&cfg, name, None, None);
        let given = meta.get("summary").and_then(Value::as_str).unwrap_or("");
        let summary = if !given.is_empty() {
            given.to_string()
        } else if let Some(url) = cfg
            .get("url")
            .filter(|u| !u.is_null() && *u != &Value::Bool(false))
        {
            format!(
                "http: {}",
                url.as_str().map_or_else(|| url.to_string(), str::to_string)
            )
        } else {
            let mut parts = vec![cfg.get("command").map_or_else(
                || "?".to_string(),
                |c| c.as_str().map_or_else(|| c.to_string(), str::to_string),
            )];
            if let Some(Value::Array(a)) = cfg.get("args") {
                parts.extend(
                    a.iter()
                        .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)),
                );
            }
            format!("stdio: {}", parts.join(" "))
        };
        let mut tags = norm_tags(meta.get("tags"));
        tags.push("mcp".into());
        let tags: Vec<String> = tags
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        out.push(Item {
            name: name.clone(),
            summary: clip(&summary),
            tags,
            content: serde_json::to_string(&cfg)?,
        });
    }
    Ok(out)
}

/// Build an item for `hub_share`. MCP content is parsed as JSON: either a
/// whole `mcp.json` (first server wins) or a single server config.
pub fn share_item(kind: &str, name: &str, content: &str) -> anyhow::Result<Item> {
    if kind == "mcp" {
        let v: Value = serde_json::from_str(content)
            .map_err(|e| anyhow::anyhow!("mcp content must be JSON: {e}"))?;
        let doc = if v.get("mcpServers").is_some() || v.get("servers").is_some() {
            v
        } else {
            let mut servers = Map::new();
            servers.insert(name.to_string(), v);
            let mut d = Map::new();
            d.insert("mcpServers".into(), Value::Object(servers));
            Value::Object(d)
        };
        mcp_items(&doc)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("mcp content has no servers"))
    } else {
        Ok(md_item(content, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn env_name_shapes() {
        assert_eq!(env_name("github-token"), "GITHUB_TOKEN");
        assert_eq!(env_name("__a..b__"), "A_B");
        assert_eq!(env_name("remote_Authorization"), "REMOTE_AUTHORIZATION");
    }

    #[test]
    fn redacts_known_shapes() {
        let t = redact_text("key sk-proj-FAKEfakeFAKEfake0000 and ghp_FAKEFAKEFAKEFAKEFAKEFAKE00");
        assert_eq!(t, "key ${OPENAI_API_KEY} and ${GITHUB_TOKEN}");
        assert_eq!(redact_text("sk-ant-FAKE000000xyz"), "${ANTHROPIC_API_KEY}");
        assert_eq!(redact_text("xoxb-FAKE-0000-fake"), "${SLACK_TOKEN}");
        assert_eq!(redact_text("AKIAIOSFODNN7EXAMPLE"), "${AWS_ACCESS_KEY_ID}");
        assert_eq!(redact_text("ah_FAKEfakeFAKEfakeFAKEfake"), "${HUB_KEY}");
        assert_eq!(
            redact_text("Authorization: Bearer abcdefghijklmnop1234"),
            "Authorization: Bearer ${BEARER_TOKEN}"
        );
        assert_eq!(
            redact_text("postgres://u:pw1@h/db"),
            "postgres://u:${URL_PASSWORD}@h/db"
        );
        assert_eq!(
            redact_text("password: hunter2hunter2"),
            "password: ${PASSWORD}"
        );
        assert_eq!(
            redact_text("\"api_key\": \"abcdef123456\""),
            "\"api_key\": ${API_KEY}"
        );
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIfake\n-----END RSA PRIVATE KEY-----";
        assert_eq!(redact_text(pem), "${PRIVATE_KEY}");
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let s = "Risk-based task-scheduling, disk-cache, password: short";
        assert_eq!(redact_text(s), s);
        assert!(!looks_secret(s));
    }

    #[test]
    fn redaction_is_idempotent() {
        let once =
            redact_text("postgres://u:pw1@h/db password=abcdefgh123 Bearer abcdefghijklmnop1234");
        assert_eq!(redact_text(&once), once);
    }

    #[test]
    fn sanitizes_mcp_config() {
        let cfg = json!({
            "command": "x",
            "args": ["--token", "plain-value", "--api-key=abc", "--name=ok", "-v"],
            "env": {"API_TOKEN": "abc", "LOG_LEVEL": "info", "OTHER": "ghp_FAKEFAKEFAKEFAKEFAKEFAKE00", "EMPTY": ""},
            "headers": {"Authorization": "Bearer zzz", "X-Api-Key": "k", "Accept": "json"}
        });
        let out = sanitize_json(&cfg, "svc", None, None);
        assert_eq!(
            out,
            json!({
                "command": "x",
                "args": ["--token", "${SVC_TOKEN}", "--api-key=${SVC_API_KEY}", "--name=ok", "-v"],
                "env": {"API_TOKEN": "${API_TOKEN}", "LOG_LEVEL": "info", "OTHER": "${OTHER}", "EMPTY": ""},
                "headers": {"Authorization": "Bearer ${SVC_AUTHORIZATION}", "X-Api-Key": "${SVC_X_API_KEY}", "Accept": "json"}
            })
        );
    }

    #[test]
    fn front_matter_variants() {
        let md = "---\nname: n\ndescription: >-\n  one\n  two\ntags:\n  - Rust\n  - \"sqlx\"\n---\nbody\n";
        let it = md_item(md, "fallback");
        assert_eq!(it.name, "n");
        assert_eq!(it.summary, "one two");
        assert_eq!(it.tags, vec!["rust", "sqlx"]);

        let it = md_item("---\ntags: [a, 'B', a]\n---\n# H\n\nFirst line.\n", "f");
        assert_eq!(it.name, "f");
        assert_eq!(it.summary, "First line.");
        assert_eq!(it.tags, vec!["a", "b"]);

        let it = md_item("no front matter\n", "f");
        assert_eq!(it.summary, "no front matter");
        assert!(it.tags.is_empty());
    }

    #[test]
    fn clips_long_summaries() {
        let long = "x".repeat(250);
        let it = md_item(&long, "f");
        assert_eq!(it.summary.chars().count(), 200);
        assert!(it.summary.ends_with("..."));
    }

    #[test]
    fn share_item_wraps_single_server() {
        let it = share_item(
            "mcp",
            "gh",
            r#"{"command":"gh-mcp","env":{"GH_TOKEN":"abc"}}"#,
        )
        .unwrap();
        assert_eq!(
            it.content,
            r#"{"command":"gh-mcp","env":{"GH_TOKEN":"${GH_TOKEN}"}}"#
        );
        assert_eq!(it.tags, vec!["mcp"]);
        assert!(share_item("mcp", "gh", "not json").is_err());
    }
}
