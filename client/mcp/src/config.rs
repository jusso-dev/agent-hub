//! Client configuration: HUB_URL, HUB_KEY and HUB_HOME, with the same
//! credentials file fallback as client/hub.sh.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    pub key: Option<String>,
    pub home: PathBuf,
}

fn read_cred(home: &std::path::Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(home.join("credentials")).ok()?;
    let prefix = format!("{name}=");
    text.lines()
        .rev()
        .find_map(|l| l.strip_prefix(&prefix))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Config {
    /// Default HUB_HOME: `${XDG_CONFIG_HOME:-$HOME/.config}/agent-hub`.
    pub fn default_home() -> PathBuf {
        if let Some(h) = env("HUB_HOME") {
            return PathBuf::from(h);
        }
        let base = env("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env("HOME").unwrap_or_else(|| ".".into())).join(".config")
            });
        base.join("agent-hub")
    }

    pub fn from_env() -> Self {
        let home = Self::default_home();
        let url = env("HUB_URL")
            .or_else(|| read_cred(&home, "HUB_URL"))
            .unwrap_or_else(|| "http://127.0.0.1:8787".into());
        let key = env("HUB_KEY").or_else(|| read_cred(&home, "HUB_KEY"));
        Self::new(url, key, home)
    }

    pub fn new(url: impl Into<String>, key: Option<String>, home: PathBuf) -> Self {
        let url = url.into().trim_end_matches('/').to_string();
        Self { url, key, home }
    }

    /// First 11 characters of the key (`ah_` plus the lookup prefix). Used to
    /// keep sync caches apart per identity without storing the key.
    pub fn key_tag(&self) -> String {
        self.key.as_deref().unwrap_or("").chars().take(11).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_credentials_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("credentials"),
            "HUB_URL=http://x:1/\nHUB_KEY=ah_old\nHUB_KEY=ah_new\n",
        )
        .unwrap();
        assert_eq!(read_cred(dir.path(), "HUB_KEY").as_deref(), Some("ah_new"));
        assert_eq!(
            read_cred(dir.path(), "HUB_URL").as_deref(),
            Some("http://x:1/")
        );
        let c = Config::new(
            "http://x:1/",
            Some("ah_12345678abcdef".into()),
            dir.path().into(),
        );
        assert_eq!(c.url, "http://x:1");
        assert_eq!(c.key_tag(), "ah_12345678");
    }
}
