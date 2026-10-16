use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;

/// UTC timestamp, fixed width so string order is time order.
pub fn now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `{prefix}` + 32 random bytes as base64url.
pub fn random_key(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// Normalise tags: trim, lowercase, drop empties and duplicates, keep order.
pub fn norm_tags<I, S>(tags: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.as_ref().trim().trim_start_matches('#').to_lowercase();
        if !t.is_empty() && t.len() <= 64 && !out.contains(&t) {
            out.push(t);
        }
    }
    out.truncate(32);
    out
}

pub fn split_tags(s: &str) -> Vec<String> {
    norm_tags(s.split(','))
}
