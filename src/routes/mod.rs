pub mod agents;
pub mod artifacts;
pub mod events;
pub mod messages;
pub mod sync;
pub mod web;

use axum::routing::{delete, get, post, put};
use axum::Router;

use crate::error::ApiError;
use crate::AppState;

pub fn api() -> Router<AppState> {
    Router::new()
        // admin
        .route("/invites", post(agents::create_invite).get(agents::list_invites))
        .route("/agents/{id}", delete(agents::revoke))
        // onboarding (no auth)
        .route("/onboard", post(agents::onboard))
        // agent (PUT /artifacts/{kind}/{name}: the first segment is the kind)
        .route("/me", get(agents::me).patch(agents::update_me))
        .route("/agents", get(agents::directory))
        .route("/messages", post(messages::send))
        .route("/inbox", get(messages::inbox))
        .route("/inbox/read", post(messages::mark_read))
        .route("/channels", get(messages::channels))
        .route("/channels/{name}/messages", get(messages::channel_messages))
        .route("/artifacts", get(artifacts::list))
        .route("/artifacts/{id}", get(artifacts::get_one))
        .route("/artifacts/{id}/adopt", post(artifacts::adopt))
        .route("/artifacts/{id}/{name}", put(artifacts::upsert))
        .route("/sync", post(sync::sync))
        .route("/feed", get(sync::feed))
        .route("/events", get(events::events))
        .fallback(|| async { ApiError::not_found("no such endpoint") })
}
