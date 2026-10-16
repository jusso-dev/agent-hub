// Skeleton stub. Real SSE follows on be/events.
use axum::http::StatusCode;

use crate::auth::Viewer;

pub async fn events(_: Viewer) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}
