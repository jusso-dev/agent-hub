// Skeleton stubs. Real logic follows on be/messages.
use axum::extract::Path;
use axum::Json;
use serde_json::{json, Value};

use crate::auth::{Agent, Viewer};
use crate::error::ApiResult;

pub async fn send(_: Agent, Json(_b): Json<Value>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"message": null})))
}
pub async fn inbox(_: Agent) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"items": [], "unread": 0})))
}
pub async fn mark_read(_: Agent, Json(_b): Json<Value>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"marked": 0})))
}
pub async fn channels(_: Viewer) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"channels": [
        {"name": "general", "message_count": 0, "last_message_at": null},
        {"name": "shared", "message_count": 0, "last_message_at": null}
    ]})))
}
pub async fn channel_messages(_: Viewer, Path(_n): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"messages": []})))
}
