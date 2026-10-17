//! Definition of done: admin invites, two agents onboard, A DMs B, B reads inbox,
//! A shares a skill tagged `rust`, B (interested in `rust`) gets a share message and a
//! sync suggestion, B adopts, A sees the adoption.
mod common;
use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn definition_of_done() {
    let h = hub().await;
    let (_a, ka) = h.onboard("alice").await;
    let (_b, kb) = h.onboard("bob").await;

    // B is interested in rust.
    let (s, me) = h
        .call(
            Method::PATCH,
            "/api/me",
            Some(&kb),
            Some(json!({"interests": ["Rust", "sqlite"]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["agent"]["interests"], json!(["rust", "sqlite"]));

    // A DMs B.
    let (s, sent) = h
        .call(
            Method::POST,
            "/api/messages",
            Some(&ka),
            Some(json!({"to": "@bob", "body": "g'day bob"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{sent}");
    let msg_id = sent["message"]["id"].as_str().unwrap().to_string();

    // B reads the inbox.
    let (s, inbox) = h
        .call(Method::GET, "/api/inbox?unread=true", Some(&kb), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(inbox["unread"], 1);
    assert_eq!(inbox["items"][0]["from"], "alice");
    assert_eq!(inbox["items"][0]["body"], "g'day bob");
    let (_, marked) = h
        .call(
            Method::POST,
            "/api/inbox/read",
            Some(&kb),
            Some(json!({"ids": [msg_id]})),
        )
        .await;
    assert_eq!(marked["marked"], 1);
    let (_, inbox) = h
        .call(Method::GET, "/api/inbox?unread=true", Some(&kb), None)
        .await;
    assert_eq!(inbox["unread"], 0);

    // A shares a skill tagged rust.
    let (s, put) = h
        .call(
            Method::PUT,
            "/api/artifacts/skill/rust-errors",
            Some(&ka),
            Some(json!({"summary": "Error handling with thiserror", "tags": ["rust"], "content": "# Rust errors\nUse thiserror.", "visibility": "shared"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{put}");
    assert_eq!(put["changed"], true);
    assert_eq!(put["artifact"]["version"], 1);
    let art_id = put["artifact"]["id"].as_str().unwrap().to_string();

    // B gets a share message.
    let (_, inbox) = h
        .call(Method::GET, "/api/inbox?unread=true", Some(&kb), None)
        .await;
    assert_eq!(inbox["unread"], 1, "{inbox}");
    let share = &inbox["items"][0];
    assert_eq!(share["kind"], "share");
    assert_eq!(share["artifact_id"], art_id.as_str());
    assert!(share["body"]
        .as_str()
        .unwrap()
        .starts_with("@alice shared skill `rust-errors` v1"));

    // The same note is in #shared.
    let (_, ch) = h
        .call(
            Method::GET,
            "/api/channels/shared/messages",
            Some(&kb),
            None,
        )
        .await;
    assert_eq!(ch["messages"].as_array().unwrap().len(), 1);

    // B gets a sync suggestion.
    let (s, sync) = h
        .call(Method::POST, "/api/sync", Some(&kb), Some(json!({})))
        .await;
    assert_eq!(s, StatusCode::OK, "{sync}");
    assert_eq!(sync["suggestions"][0]["id"], art_id.as_str());
    assert_eq!(sync["suggestions"][0]["matched_tags"], json!(["rust"]));

    // B learns it and adopts.
    let (s, full) = h
        .call(
            Method::GET,
            &format!("/api/artifacts/{art_id}"),
            Some(&kb),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(full["artifact"]["content"]
        .as_str()
        .unwrap()
        .contains("thiserror"));
    let (s, adopt) = h
        .call(
            Method::POST,
            &format!("/api/artifacts/{art_id}/adopt"),
            Some(&kb),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{adopt}");

    // No longer suggested.
    let (_, sync) = h
        .call(Method::POST, "/api/sync", Some(&kb), Some(json!({})))
        .await;
    assert_eq!(sync["suggestions"], json!([]));

    // A sees the adoption: in the inbox and on the artifact.
    let (_, inbox) = h
        .call(Method::GET, "/api/inbox?unread=true", Some(&ka), None)
        .await;
    assert_eq!(inbox["items"][0]["kind"], "system");
    assert!(inbox["items"][0]["body"]
        .as_str()
        .unwrap()
        .contains("@bob adopted your skill `rust-errors` v1"));
    let (_, full) = h
        .call(
            Method::GET,
            &format!("/api/artifacts/{art_id}"),
            Some(&ka),
            None,
        )
        .await;
    assert_eq!(full["adoptions"][0]["handle"], "bob");
    assert_eq!(full["artifact"]["adoption_count"], 1);
}

#[tokio::test]
async fn versions_sync_and_secrets() {
    let h = hub().await;
    let (_, ka) = h.onboard("alice").await;
    let body = json!({"skills": [{"name": "deploy", "summary": "How to deploy", "tags": "ops,docker", "content": "v1"}],
                      "memories": [{"name": "notes", "content": "private stuff"}]});
    let (s, r) = h
        .call(Method::POST, "/api/sync", Some(&ka), Some(body.clone()))
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["changed"].as_array().unwrap().len(), 2);
    let (_, r) = h
        .call(Method::POST, "/api/sync", Some(&ka), Some(body))
        .await;
    assert_eq!(r["unchanged"], 2);
    let (_, r) = h
        .call(
            Method::POST,
            "/api/sync",
            Some(&ka),
            Some(json!({"skills": [{"name": "deploy", "tags": "ops", "content": "v2"}]})),
        )
        .await;
    assert_eq!(r["changed"][0]["version"], 2);
    let id = r["changed"][0]["id"].as_str().unwrap();
    let (_, full) = h
        .call(
            Method::GET,
            &format!("/api/artifacts/{id}"),
            Some(&ka),
            None,
        )
        .await;
    assert_eq!(full["versions"].as_array().unwrap().len(), 2);

    // Memories default to private: another agent cannot see them.
    let (_, kb) = h.onboard("bob").await;
    let (_, list) = h
        .call(Method::GET, "/api/artifacts?kind=memory", Some(&kb), None)
        .await;
    assert_eq!(list["artifacts"], json!([]));
    let (_, list) = h
        .call(Method::GET, "/api/artifacts?q=deploy", Some(&kb), None)
        .await;
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1);

    // Secrets in MCP configs are rejected and nothing is written.
    let leaky = json!({"skills": [{"name": "new-one", "content": "x"}],
                       "mcps": [{"name": "github", "content": "{\"env\":{\"GITHUB_TOKEN\":\"ghp_abcdefghijklmnopqrstuvwxyz\"}}"}]});
    let (s, err) = h
        .call(Method::POST, "/api/sync", Some(&ka), Some(leaky))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(err["hint"].as_str().unwrap().contains("${ENV_VAR}"));
    let (_, list) = h
        .call(Method::GET, "/api/artifacts?q=new-one", Some(&ka), None)
        .await;
    assert_eq!(list["artifacts"], json!([]));
    let (s, _) = h
        .call(
            Method::PUT,
            "/api/artifacts/mcp/github",
            Some(&ka),
            Some(json!({"content": "{\"password\": \"hunter2\"}"})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = h
        .call(
            Method::PUT,
            "/api/artifacts/mcp/github",
            Some(&ka),
            Some(json!({"content": "{\"token\": \"${GITHUB_TOKEN}\"}"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn channels_feed_and_limits() {
    let h = hub().await;
    let (_, ka) = h.onboard("alice").await;
    let (_, kb) = h.onboard("bob").await;
    let (_, chans) = h.call(Method::GET, "/api/channels", Some(&ka), None).await;
    let names: Vec<&str> = chans["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["general", "shared"]);

    let (s, _) = h
        .call(
            Method::POST,
            "/api/messages",
            Some(&ka),
            Some(json!({"to": "#general", "body": "hi all, @bob have a look"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, inbox) = h.call(Method::GET, "/api/inbox", Some(&kb), None).await;
    assert_eq!(inbox["items"][0]["channel"], "general");
    let (_, feed) = h.call(Method::GET, "/api/feed", Some(&kb), None).await;
    assert_eq!(feed["messages"].as_array().unwrap().len(), 1);
    assert_eq!(feed["agents"].as_array().unwrap().len(), 2);
    let (_, feed) = h
        .call(
            Method::GET,
            &format!("/api/feed?since={}", feed["now"].as_str().unwrap()),
            Some(&kb),
            None,
        )
        .await;
    assert_eq!(feed["messages"], json!([]));

    // Admin can read the directory too. It lists the built-in @admin as well.
    let (s, dir) = h.call(Method::GET, "/api/agents", Some(ADMIN), None).await;
    assert_eq!(s, StatusCode::OK);
    let handles: Vec<&str> = dir["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["handle"].as_str().unwrap())
        .collect();
    assert_eq!(handles, ["admin", "alice", "bob"]);

    // Body limit: 256 KB.
    let big = "x".repeat(300 * 1024);
    let (s, _) = h
        .call(
            Method::POST,
            "/api/messages",
            Some(&ka),
            Some(json!({"to": "#general", "body": big})),
        )
        .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn rate_limit_and_sse() {
    let h = hub().await;
    let (_, ka) = h.onboard("alice").await;
    let mut last = StatusCode::OK;
    for _ in 0..61 {
        last = h.call(Method::GET, "/api/me", Some(&ka), None).await.0;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);

    use axum::body::Body;
    use tower::ServiceExt;
    let req = axum::http::Request::get("/api/events")
        .header("authorization", format!("Bearer {ADMIN}"))
        .body(Body::empty())
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "text/event-stream");
}

#[tokio::test]
async fn revoked_agent_stream_closes() {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let h = hub().await;
    let (alice, ka) = h.onboard("alice").await;
    let req = axum::http::Request::get("/api/events")
        .header("authorization", format!("Bearer {ka}"))
        .body(Body::empty())
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let id = alice["id"].as_str().unwrap().to_string();
    let reader = tokio::spawn(async move { res.into_body().collect().await.unwrap().to_bytes() });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (s, r) = h
        .call(
            Method::DELETE,
            &format!("/api/agents/{id}"),
            Some(ADMIN),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(5), reader)
        .await
        .expect("stream should close after revoke")
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("event: ready"), "{text}");
    assert!(text.contains("event: revoked"), "{text}");
    // The revoked key can't reopen it.
    let (s, _) = h.call(Method::GET, "/api/events", Some(&ka), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn shared_items_are_scanned_for_secrets() {
    let h = hub().await;
    let (_a, ka) = h.onboard("alice").await;
    let leak = "Deploy with token ghp_abcdefghijklmnopqrstuvwxyz0123456789";

    // A shared skill (the default for skills) is refused.
    let (s, r) = h
        .call(
            Method::PUT,
            "/api/artifacts/skill/deploy",
            Some(&ka),
            Some(json!({"summary":"s","content":leak})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{r}");
    assert!(r["error"].as_str().unwrap().contains("deploy"), "{r}");

    // A memory explicitly shared is refused too.
    let (s, _) = h
        .call(
            Method::PUT,
            "/api/artifacts/memory/note",
            Some(&ka),
            Some(json!({"summary":"s","content":leak,"visibility":"shared"})),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // A private memory (the default) is allowed: only its owner can read it.
    let (s, r) = h
        .call(
            Method::PUT,
            "/api/artifacts/memory/note",
            Some(&ka),
            Some(json!({"summary":"s","content":leak})),
        )
        .await;
    assert!(s.is_success(), "{s} {r}");

    // A sync with one leaky shared memory writes nothing at all.
    let (s, r) = h
        .call(
            Method::POST,
            "/api/sync",
            Some(&ka),
            Some(json!({
                "memories":[{"name":"leaky","summary":"s","content":leak,"visibility":"shared"}],
                "skills":[{"name":"clean-skill","summary":"s","content":"no secrets here"}],
                "mcps":[]
            })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{r}");
    assert!(r["error"].as_str().unwrap().contains("leaky"), "{r}");
    let (_, list) = h.call(Method::GET, "/api/artifacts", Some(&ka), None).await;
    assert!(!list.to_string().contains("clean-skill"), "{list}");
    assert!(list.to_string().contains("note"), "{list}");
}
