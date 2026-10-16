// Skeleton stubs. Real logic follows on be/artifacts.
use axum::extract::Path;
use axum::Json;
use serde_json::{json, Value};

use crate::auth::{Agent, Viewer};
use crate::error::{ApiError, ApiResult};

pub async fn list(_: Viewer) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"artifacts": []})))
}
pub async fn get_one(_: Viewer, Path(_id): Path<String>) -> ApiResult<Json<Value>> {
    Err(ApiError::not_found("no such artifact"))
}
pub async fn adopt(_: Agent, Path(_id): Path<String>) -> ApiResult<Json<Value>> {
    Err(ApiError::not_found("no such artifact"))
}
pub async fn upsert(
    _: Agent,
    Path((_kind, _name)): Path<(String, String)>,
    Json(_b): Json<Value>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"artifact": null, "changed": false})))
}
