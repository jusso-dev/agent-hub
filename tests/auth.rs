mod common;
use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn invite_onboard_and_me() {
    let h = hub().await;
    let (agent, key) = h.onboard("alpha").await;
    assert!(key.starts_with("ah_"));
    assert_eq!(agent["handle"], "alpha");

    let (s, me) = h.call(Method::GET, "/api/me", Some(&key), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["agent"]["handle"], "alpha");

    // Bad keys and wrong roles.
    let (s, _) = h.call(Method::GET, "/api/me", Some("ah_nope"), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = h.call(Method::GET, "/api/me", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = h.call(Method::GET, "/api/invites", Some(&key), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invite_is_single_use_and_revoke_works() {
    let h = hub().await;
    let (_, inv) = h
        .call(Method::POST, "/api/invites", Some(ADMIN), Some(json!({"note": "x", "owner": "justin"})))
        .await;
    let body = json!({"invite_key": inv["invite_key"], "handle": "one", "runtime": "codex"});
    let (s, ob) = h.call(Method::POST, "/api/onboard", None, Some(body.clone())).await;
    assert_eq!(s, StatusCode::CREATED);
    let mut again = body;
    again["handle"] = json!("two");
    let (s, _) = h.call(Method::POST, "/api/onboard", None, Some(again)).await;
    assert_eq!(s, StatusCode::CONFLICT);

    let (_, list) = h.call(Method::GET, "/api/invites", Some(ADMIN), None).await;
    assert!(list["invites"][0]["redeemed_at"].is_string());

    let key = ob["agent_key"].as_str().unwrap();
    let id = ob["agent"]["id"].as_str().unwrap();
    let (s, _) = h.call(Method::GET, "/api/me", Some(key), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = h.call(Method::DELETE, &format!("/api/agents/{id}"), Some(ADMIN), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = h.call(Method::GET, "/api/me", Some(key), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}
