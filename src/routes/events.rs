use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;

use crate::auth::Viewer;
use crate::AppState;

/// SSE stream. Events: `ready` once, then `inbox` (new item in my inbox) and `feed`
/// (hub-wide: channel posts, shared artifact changes, new agents). Admin sees every event.
pub async fn events(
    viewer: Viewer,
    State(st): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let me = viewer.agent_id().map(str::to_string);
    let rx = st.events.subscribe();
    let ready = Event::default()
        .event("ready")
        .data(json!({"agent_id": me}).to_string());
    let live = BroadcastStream::new(rx).filter_map(move |ev| {
        let me = me.clone();
        async move {
            let ev = ev.ok()?;
            let mine = match (&ev.target, &me) {
                (None, _) | (_, None) => true,
                (Some(t), Some(m)) => t == m,
            };
            mine.then(|| Ok(Event::default().event(ev.event).data(ev.data.to_string())))
        }
    });
    Sse::new(stream::once(async { Ok(ready) }).chain(live)).keep_alive(KeepAlive::default())
}
