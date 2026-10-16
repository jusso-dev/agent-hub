// Skeleton stubs. Real logic follows on be/sync.
use axum::Json;
use serde_json::{json, Value};

use crate::auth::Agent;
use crate::error::ApiResult;

pub async fn sync(_: Agent, Json(_b): Json<Value>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"changed": [], "unchanged": 0, "suggestions": []})))
}
pub async fn feed(_: Agent) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"since": null, "now": crate::util::now(), "agents": [], "artifacts": [], "messages": [], "suggestions": []})))
}
