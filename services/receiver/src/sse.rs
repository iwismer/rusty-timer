use crate::control_api::AppState;
use crate::ui_events::ReceiverUiEvent;
use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};

pub async fn receiver_sse(
    State(state): State<Arc<AppState>>,
) -> Sse<impl futures_util::stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.ui_tx.subscribe();
    let updates = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(event) => {
            let event_type = match &event {
                ReceiverUiEvent::StatusChanged { .. } => "status_changed",
                ReceiverUiEvent::StreamsSnapshot { .. } => "streams_snapshot",
                ReceiverUiEvent::LogEntry { .. } => "log_entry",
                ReceiverUiEvent::UpdateStatusChanged { .. } => "update_status_changed",
                ReceiverUiEvent::StreamCountsUpdated { .. } => "stream_counts_updated",
                ReceiverUiEvent::ModeChanged { .. } => "mode_changed",
            };
            match serde_json::to_string(&event) {
                Ok(json) => Some(Ok(Event::default().event(event_type).data(json))),
                Err(_) => None,
            }
        }
        Err(_) => Some(Ok(Event::default().event("resync").data("{}"))),
    });
    let initial = tokio_stream::once(Ok(Event::default().event("connected").data("{}")));
    let stream = initial.chain(updates);

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}
