use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::future;
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;

use crate::auth::StreamViewer;
use crate::routes::agents::REVOKED_EVENT;
use crate::AppState;

/// SSE stream. Events: `ready` once, then `inbox` (new item in my inbox) and `feed`
/// (hub-wide: channel posts, shared artifact changes, new agents). Admin sees every event.
/// Not rate limited: one call holds the stream open.
/// When the caller's key is revoked the stream sends `revoked` and closes.
pub async fn events(
    StreamViewer(viewer): StreamViewer,
    State(st): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let me = viewer.agent_id().map(str::to_string);
    let rx = st.events.subscribe();
    let ready = Event::default()
        .event("ready")
        .data(json!({"agent_id": me}).to_string());
    let me_stop = me.clone();
    let live = BroadcastStream::new(rx)
        .filter_map(move |ev| {
            let me = me.clone();
            async move {
                let ev = ev.ok()?;
                let mine = match (&ev.target, &me) {
                    (None, _) | (_, None) => true,
                    (Some(t), Some(m)) => t == m,
                };
                mine.then_some(ev)
            }
        })
        // Pass the `revoked` event itself through, then end the stream at once.
        .flat_map(move |ev| {
            let last = ev.event == REVOKED_EVENT && me_stop.is_some() && ev.target == me_stop;
            let mut items = vec![Some(ev)];
            if last {
                items.push(None);
            }
            stream::iter(items)
        })
        .take_while(|ev| future::ready(ev.is_some()))
        .filter_map(|ev| {
            future::ready(
                ev.map(|ev| Ok(Event::default().event(ev.event).data(ev.data.to_string()))),
            )
        });
    Sse::new(stream::once(async { Ok(ready) }).chain(live)).keep_alive(KeepAlive::default())
}
