//! Client behaviour against a stub HTTP server.

use std::path::PathBuf;

use hub_mcp::client::HubClient;
use hub_mcp::config::Config;
use hub_mcp::ops;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn client(url: &str, home: &std::path::Path) -> HubClient {
    HubClient::new(Config::new(
        url,
        Some("ah_testkey0000000000000000".into()),
        home.into(),
    ))
    .unwrap()
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test/fixtures/agent")
}

#[tokio::test]
async fn sync_sends_only_changes_and_caches() {
    let srv = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync"))
        .and(header("authorization", "Bearer ah_testkey0000000000000000"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"changed": [], "unchanged": 0, "suggestions": []})),
        )
        .expect(2)
        .mount(&srv)
        .await;
    let home = tempfile::tempdir().unwrap();
    let c = client(&srv.uri(), home.path());

    let first = ops::sync_dir(&c, &fixture_dir(), false, false)
        .await
        .unwrap();
    assert_eq!(first["sent"], 8);
    let second = ops::sync_dir(&c, &fixture_dir(), false, false)
        .await
        .unwrap();
    assert_eq!(second["sent"], 0);

    let reqs: Vec<Request> = srv.received_requests().await.unwrap();
    let b1: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    let b2: Value = serde_json::from_slice(&reqs[1].body).unwrap();
    assert_eq!(b1["mcps"].as_array().unwrap().len(), 4);
    assert_eq!(b2, json!({"memories": [], "skills": [], "mcps": []}));

    // Dry run with force shows everything again without calling the server.
    let dry = ops::sync_dir(&c, &fixture_dir(), true, true).await.unwrap();
    assert_eq!(dry["sent"], 8);
    assert_eq!(srv.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn failed_sync_does_not_update_cache() {
    let srv = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "error": "mcp `github` looks like it holds a secret",
            "hint": "use ${ENV_VAR} placeholders"
        })))
        .mount(&srv)
        .await;
    let home = tempfile::tempdir().unwrap();
    let c = client(&srv.uri(), home.path());
    let err = ops::sync_dir(&c, &fixture_dir(), false, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("HTTP 422"), "{err}");
    assert!(err.contains("looks like it holds a secret"), "{err}");
    assert!(err.contains("placeholders"), "{err}");
    assert!(!home.path().join("sync-cache").exists());
}

#[tokio::test]
async fn inbox_marks_unread_as_read() {
    let srv = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/inbox"))
        .and(query_param("unread", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "m1", "body": "hi", "read_at": null}, {"id": "m2", "body": "old", "read_at": "x"}],
            "unread": 1
        })))
        .mount(&srv)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/inbox/read"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"marked": 1})))
        .expect(1)
        .mount(&srv)
        .await;
    let home = tempfile::tempdir().unwrap();
    let c = client(&srv.uri(), home.path());
    ops::inbox(&c, true, 50, true).await.unwrap();
    let reqs = srv.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&reqs[1].body).unwrap();
    assert_eq!(body, json!({"ids": ["m1"]}));
}

#[tokio::test]
async fn share_strips_and_refuses() {
    let srv = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/artifacts/mcp/gh"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"artifact": {"id": "a1", "version": 1}, "changed": true})),
        )
        .mount(&srv)
        .await;
    let home = tempfile::tempdir().unwrap();
    let c = client(&srv.uri(), home.path());
    let tags = vec!["GitHub".to_string()];
    ops::share(
        &c,
        "mcp",
        "gh",
        Some(r#"{"command":"gh","env":{"GH_TOKEN":"ghp_FAKEFAKEFAKEFAKEFAKEFAKE00"}}"#),
        None,
        None,
        &tags,
        false,
    )
    .await
    .unwrap();
    let reqs = srv.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(
        body["content"],
        r#"{"command":"gh","env":{"GH_TOKEN":"${GH_TOKEN}"}}"#
    );
    assert_eq!(body["tags"], json!(["github", "mcp"]));
    assert_eq!(body["visibility"], "shared");

    let err = ops::share(
        &c,
        "skill",
        "x",
        Some("-----BEGIN OPENSSH PRIVATE KEY-----\nabc"),
        None,
        None,
        &[],
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("secret"));
    assert!(
        ops::share(&c, "bogus", "x", Some("y"), None, None, &[], false)
            .await
            .is_err()
    );
    assert_eq!(srv.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn feed_remembers_now() {
    let srv = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/feed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"since": null, "now": "2026-10-16T06:00:00.000000Z", "artifacts": []}),
        ))
        .mount(&srv)
        .await;
    let home = tempfile::tempdir().unwrap();
    let c = client(&srv.uri(), home.path());
    ops::feed(&c, None, false).await.unwrap();
    ops::feed(&c, None, false).await.unwrap();
    let reqs = srv.received_requests().await.unwrap();
    assert_eq!(reqs[0].url.query(), None);
    assert_eq!(
        reqs[1].url.query(),
        Some("since=2026-10-16T06%3A00%3A00.000000Z")
    );
}

#[tokio::test]
async fn missing_key_is_a_clear_error() {
    let home = tempfile::tempdir().unwrap();
    let c = HubClient::new(Config::new("http://127.0.0.1:9", None, home.path().into())).unwrap();
    let err = ops::adopt(&c, "x").await.unwrap_err().to_string();
    assert!(err.contains("no agent key"), "{err}");
}

#[tokio::test]
async fn get_with_version_query() {
    let srv = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/artifacts/a1"))
        .and(query_param("version", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "artifact": {"id": "a1", "kind": "skill", "name": "x", "version": 3},
            "version": {"version": 2, "summary": "old", "content": "v2 body", "created_at": "t"}
        })))
        .expect(1)
        .mount(&srv)
        .await;
    let home = tempfile::tempdir().unwrap();
    let c = client(&srv.uri(), home.path());
    let res = ops::get(&c, "a1", Some(2)).await.unwrap();
    assert_eq!(res["version"]["content"], "v2 body");
}
