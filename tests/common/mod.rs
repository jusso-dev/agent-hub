#![allow(dead_code)]
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

pub const ADMIN: &str = "test-admin-key";

pub struct Hub {
    pub app: Router,
    pub state: agent_hub::AppState,
    _dir: tempfile::TempDir,
}

pub async fn hub() -> Hub {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");
    let pool = agent_hub::db::connect(path.to_str().unwrap()).await.unwrap();
    let state = agent_hub::AppState::new(pool, ADMIN.into());
    Hub {
        app: agent_hub::app(state.clone()),
        state,
        _dir: dir,
    }
}

impl Hub {
    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(k) = key {
            req = req.header("authorization", format!("Bearer {k}"));
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    /// Admin creates an invite and an agent redeems it. Returns (agent json, key).
    pub async fn onboard(&self, handle: &str) -> (Value, String) {
        let (s, inv) = self
            .call(
                Method::POST,
                "/api/invites",
                Some(ADMIN),
                Some(serde_json::json!({"note": "test", "owner": "justin"})),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{inv}");
        let (s, ob) = self
            .call(
                Method::POST,
                "/api/onboard",
                None,
                Some(serde_json::json!({
                    "invite_key": inv["invite_key"],
                    "handle": handle,
                    "display_name": handle,
                    "runtime": "claude",
                    "owner": "justin",
                    "description": "test agent"
                })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{ob}");
        (ob["agent"].clone(), ob["agent_key"].as_str().unwrap().to_string())
    }
}
