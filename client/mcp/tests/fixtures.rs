//! The Rust sanitiser must produce exactly what client/hub.sh produces for
//! the shared fixture directory.

use std::collections::BTreeMap;
use std::path::PathBuf;

use hub_mcp::sync::{collect, plan};
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test/fixtures")
}

#[test]
fn sync_payload_matches_hub_sh() {
    let c = collect(&fixtures().join("agent")).unwrap();
    assert!(c.skipped.is_empty(), "skipped: {:?}", c.skipped);
    let p = plan(&c, &BTreeMap::new());
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("expected_sync.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(p.payload, expected);
    assert_eq!(p.sent, 8);
    assert_eq!(p.total, 8);
}

#[test]
fn no_fixture_secret_survives() {
    let c = collect(&fixtures().join("agent")).unwrap();
    let all = serde_json::to_string(&plan(&c, &BTreeMap::new()).payload).unwrap();
    for needle in [
        "ghp_FAKE",
        "sk-proj",
        "hunter2",
        "hooks.slack.com",
        "s3cretpass",
        "AKIAIOSFODNN7EXAMPLE",
        "ah_FAKE",
        "xoxb-",
        "abc123def456",
        "FAKEfakeFAKEfake0000abcd",
    ] {
        assert!(!all.contains(needle), "{needle} leaked");
    }
}

#[test]
fn unchanged_items_are_not_resent() {
    let c = collect(&fixtures().join("agent")).unwrap();
    let first = plan(&c, &BTreeMap::new());
    let second = plan(&c, &first.all);
    assert_eq!(second.sent, 0);
    assert_eq!(second.payload["skills"], serde_json::json!([]));
    let mut old = first.all.clone();
    old.insert("skills/plain".into(), "stale".into());
    let third = plan(&c, &old);
    assert_eq!(third.sent, 1);
    assert_eq!(third.payload["skills"][0]["name"], "plain");
}

#[test]
fn residual_secrets_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("memories")).unwrap();
    // An unterminated PEM block cannot be cut out cleanly, so it must be held back.
    std::fs::write(
        dir.path().join("memories/bad.md"),
        "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("memories/good.md"), "fine\n").unwrap();
    let c = collect(dir.path()).unwrap();
    assert_eq!(c.skipped, vec!["memories/bad".to_string()]);
    assert_eq!(c.items.len(), 1);
}
