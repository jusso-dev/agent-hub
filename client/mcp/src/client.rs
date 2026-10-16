//! Thin HTTP client for the agent-hub API.

use anyhow::{anyhow, bail, Result};
use reqwest::Method;
use serde_json::Value;

use crate::config::Config;

#[derive(Clone)]
pub struct HubClient {
    http: reqwest::Client,
    pub cfg: Config,
}

/// Pull the list out of a response that is either a bare array or a wrapper
/// such as `{"items": [...]}` or `{"artifacts": [...]}`.
pub fn as_list(v: &Value) -> Vec<Value> {
    if let Value::Array(a) = v {
        return a.clone();
    }
    for k in [
        "items",
        "messages",
        "artifacts",
        "agents",
        "invites",
        "channels",
        "data",
    ] {
        if let Some(Value::Array(a)) = v.get(k) {
            return a.clone();
        }
    }
    vec![]
}

impl HubClient {
    pub fn new(cfg: Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("hub-mcp/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http, cfg })
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let key = self
            .cfg
            .key
            .as_deref()
            .ok_or_else(|| anyhow!("no agent key: set HUB_KEY or run `hub onboard` first"))?;
        let url = format!("{}/api{}", self.cfg.url, path);
        let mut req = self
            .http
            .request(method.clone(), &url)
            .bearer_auth(key)
            .header("Accept", "application/json");
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req
            .send()
            .await
            .map_err(|e| anyhow!("cannot reach {}: {e}", self.cfg.url))?;
        let status = res.status();
        let text = res.text().await?;
        let parsed: Value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text.clone()))
        };
        if !status.is_success() {
            let msg = parsed
                .get("error")
                .or_else(|| parsed.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| text.chars().take(500).collect());
            let hint = parsed
                .get("hint")
                .and_then(Value::as_str)
                .map(|h| format!(" ({h})"))
                .unwrap_or_default();
            bail!(
                "HTTP {} from {} /api{}: {msg}{hint}",
                status.as_u16(),
                method,
                path
            );
        }
        Ok(parsed)
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.request(Method::GET, path, query, None).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.request(Method::POST, path, &[], Some(body)).await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value> {
        self.request(Method::PUT, path, &[], Some(body)).await
    }
}

/// Percent-encode one path segment.
pub fn seg(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn list_shapes() {
        assert_eq!(as_list(&json!([1, 2])).len(), 2);
        assert_eq!(as_list(&json!({"items": [1], "unread": 1})).len(), 1);
        assert_eq!(as_list(&json!({"artifacts": [1, 2, 3]})).len(), 3);
        assert!(as_list(&json!({"x": 1})).is_empty());
    }

    #[test]
    fn encodes_segments() {
        assert_eq!(seg("a b/c"), "a%20b%2Fc");
        assert_eq!(seg("sqlx-migrations"), "sqlx-migrations");
    }
}
