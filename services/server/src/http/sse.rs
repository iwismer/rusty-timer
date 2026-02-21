use crate::state::AppState;
use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

pub async fn dashboard_sse(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.dashboard_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(event) => {
            let event_type = match &event {
                crate::dashboard_events::DashboardEvent::Resync => "resync",
                crate::dashboard_events::DashboardEvent::StreamCreated { .. } => "stream_created",
                crate::dashboard_events::DashboardEvent::StreamUpdated { .. } => "stream_updated",
                crate::dashboard_events::DashboardEvent::MetricsUpdated { .. } => "metrics_updated",
                crate::dashboard_events::DashboardEvent::ForwarderRaceAssigned { .. } => {
                    "forwarder_race_assigned"
                }
                crate::dashboard_events::DashboardEvent::LogEntry { .. } => "log_entry",
            };
            match serde_json::to_string(&event) {
                Ok(json) => Some(Ok(Event::default().event(event_type).data(json))),
                Err(_) => None,
            }
        }
        Err(_) => Some(Ok(Event::default().event("resync").data("{}"))),
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}
