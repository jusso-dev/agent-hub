mod common;
use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn artifact_versions_carry_content() {
    let h = hub().await;
    let (_, ka) = h.onboard("alice").await;
    for c in ["first body", "second body"] {
        let (s, _) = h
            .call(
                Method::PUT,
                "/api/artifacts/skill/notes",
                Some(&ka),
                Some(json!({"summary": "s", "content": c})),
            )
            .await;
        assert_eq!(s, StatusCode::OK);
    }
    let (_, list) = h
        .call(Method::GET, "/api/artifacts?q=notes", Some(&ka), None)
        .await;
    let id = list["artifacts"][0]["id"].as_str().unwrap().to_string();

    let (s, full) = h
        .call(
            Method::GET,
            &format!("/api/artifacts/{id}"),
            Some(&ka),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(full["artifact"]["content"], "second body");
    assert_eq!(full["versions"][0]["version"], 2);
    assert_eq!(full["versions"][0]["content"], "second body");
    assert_eq!(full["versions"][1]["content"], "first body");
    assert!(full["adoptions"].is_array());

    let (s, one) = h
        .call(
            Method::GET,
            &format!("/api/artifacts/{id}?version=1"),
            Some(&ka),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(one["version"]["version"], 1);
    assert_eq!(one["version"]["content"], "first body");
    assert_eq!(one["artifact"]["version"], 2);

    let (s, _) = h
        .call(
            Method::GET,
            &format!("/api/artifacts/{id}?version=9"),
            Some(&ka),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dm_history() {
    let h = hub().await;
    let (_, ka) = h.onboard("alice").await;
    let (_, kb) = h.onboard("bob").await;
    let (_, kc) = h.onboard("carol").await;
    for (k, to, body) in [
        (&ka, "@bob", "one"),
        (&kb, "@alice", "two"),
        (&ka, "@bob", "three"),
        (&kc, "@alice", "not bob"),
    ] {
        let (s, _) = h
            .call(
                Method::POST,
                "/api/messages",
                Some(k),
                Some(json!({"to": to, "body": body})),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED);
    }
    h.call(
        Method::POST,
        "/api/messages",
        Some(&ka),
        Some(json!({"to": "#general", "body": "channel, not a DM"})),
    )
    .await;

    let (s, dms) = h
        .call(Method::GET, "/api/dms/bob/messages", Some(&ka), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(dms["with"], "bob");
    let bodies: Vec<&str> = dms["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["three", "two", "one"]);

    // Same thread from the other side, paged.
    let (_, page) = h
        .call(
            Method::GET,
            "/api/dms/@alice/messages?limit=2",
            Some(&kb),
            None,
        )
        .await;
    let msgs = page["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    let before = msgs[1]["created_at"].as_str().unwrap();
    let (_, older) = h
        .call(
            Method::GET,
            &format!("/api/dms/alice/messages?before={before}"),
            Some(&kb),
            None,
        )
        .await;
    assert_eq!(older["messages"][0]["body"], "one");
    assert_eq!(older["messages"].as_array().unwrap().len(), 1);

    let (s, _) = h
        .call(Method::GET, "/api/dms/nobody/messages", Some(&ka), None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_posts_as_admin_agent() {
    let h = hub().await;
    let (alice, ka) = h.onboard("alice").await;

    let (s, posted) = h
        .call(
            Method::POST,
            "/api/messages",
            Some(ADMIN),
            Some(json!({"to": "#general", "body": "welcome, @alice"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{posted}");
    assert_eq!(posted["message"]["from"], "admin");
    assert_eq!(posted["message"]["from_agent"], "admin");

    let (s, _) = h
        .call(
            Method::POST,
            "/api/messages",
            Some(ADMIN),
            Some(json!({"to": "@alice", "body": "a word from admin"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, inbox) = h.call(Method::GET, "/api/inbox", Some(&ka), None).await;
    assert_eq!(inbox["items"][0]["from"], "admin");
    assert_eq!(inbox["unread"], 2);

    // Alice can reply to @admin and the admin key reads that DM thread.
    let (s, _) = h
        .call(
            Method::POST,
            "/api/messages",
            Some(&ka),
            Some(json!({"to": "@admin", "body": "thanks"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, dms) = h
        .call(Method::GET, "/api/dms/alice/messages", Some(ADMIN), None)
        .await;
    assert_eq!(dms["messages"][0]["body"], "thanks");
    assert_eq!(dms["messages"].as_array().unwrap().len(), 2);

    // Admin-only routes unchanged; @admin cannot be revoked; agents still cannot make invites.
    let (s, _) = h.call(Method::GET, "/api/invites", Some(&ka), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = h
        .call(Method::DELETE, "/api/agents/admin", Some(ADMIN), None)
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = h
        .call(
            Method::DELETE,
            &format!("/api/agents/{}", alice["id"].as_str().unwrap()),
            Some(ADMIN),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // Nobody can onboard as @admin.
    let (_, inv) = h
        .call(Method::POST, "/api/invites", Some(ADMIN), Some(json!({})))
        .await;
    let (s, _) = h
        .call(
            Method::POST,
            "/api/onboard",
            None,
            Some(json!({"invite_key": inv["invite_key"], "handle": "admin", "runtime": "other"})),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn rate_limits_admin_and_events() {
    let h = hub().await;
    let (_, ka) = h.onboard("alice").await;
    for _ in 0..60 {
        assert_eq!(
            h.call(Method::GET, "/api/me", Some(&ka), None).await.0,
            StatusCode::OK
        );
    }
    assert_eq!(
        h.call(Method::GET, "/api/me", Some(&ka), None).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );

    // The SSE stream is not counted and still opens once the agent is limited.
    use axum::body::Body;
    use tower::ServiceExt;
    for _ in 0..3 {
        let req = axum::http::Request::get("/api/events")
            .header("authorization", format!("Bearer {ka}"))
            .body(Body::empty())
            .unwrap();
        let res = h.app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    // Admin gets 600 per minute. One call already went on the invite in onboard().
    assert_eq!(h.state.admin_rate_limit_per_min, 600);
    for i in 1..600 {
        let s = h
            .call(Method::GET, "/api/channels", Some(ADMIN), None)
            .await
            .0;
        assert_eq!(s, StatusCode::OK, "admin call {i}");
    }
    assert_eq!(
        h.call(Method::GET, "/api/channels", Some(ADMIN), None)
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
}
