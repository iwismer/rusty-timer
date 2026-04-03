use crate::state::AppState;
use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};

fn dashboard_event_type(event: &crate::dashboard_events::DashboardEvent) -> &'static str {
    match event {
        crate::dashboard_events::DashboardEvent::Resync => "resync",
        crate::dashboard_events::DashboardEvent::StreamCreated { .. } => "stream_created",
        crate::dashboard_events::DashboardEvent::StreamUpdated { .. } => "stream_updated",
        crate::dashboard_events::DashboardEvent::MetricsUpdated { .. } => "metrics_updated",
        crate::dashboard_events::DashboardEvent::ForwarderMetricsUpdated { .. } => {
            "forwarder_metrics_updated"
        }
        crate::dashboard_events::DashboardEvent::ForwarderRaceAssigned { .. } => {
            "forwarder_race_assigned"
        }
        crate::dashboard_events::DashboardEvent::ReaderInfoUpdated { .. } => "reader_info_updated",
        crate::dashboard_events::DashboardEvent::ReaderDownloadProgress { .. } => {
            "reader_download_progress"
        }
        crate::dashboard_events::DashboardEvent::ForwarderUpsUpdated { .. } => {
            "forwarder_ups_updated"
        }
        crate::dashboard_events::DashboardEvent::LogEntry { .. } => "log_entry",
    }
}

async fn cached_dashboard_events(state: &AppState) -> Vec<crate::dashboard_events::DashboardEvent> {
    let cache = state.forwarder_ups_cache.read().await;
    let mut forwarder_ids: Vec<_> = cache.keys().cloned().collect();
    forwarder_ids.sort();

    forwarder_ids
        .into_iter()
        .filter_map(|forwarder_id| {
            cache.get(&forwarder_id).map(|cached| {
                crate::dashboard_events::DashboardEvent::ForwarderUpsUpdated {
                    forwarder_id,
                    available: cached.available,
                    status: cached.status.clone(),
                }
            })
        })
        .collect()
}

pub async fn dashboard_sse(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let initial_events = cached_dashboard_events(&state).await;
    let rx = state.dashboard_tx.subscribe();
    let initial_stream = tokio_stream::iter(initial_events).filter_map(|event| {
        let event_type = dashboard_event_type(&event);
        match serde_json::to_string(&event) {
            Ok(json) => Some(Ok(Event::default().event(event_type).data(json))),
            Err(_) => None,
        }
    });
    let live_stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(event) => {
            let event_type = dashboard_event_type(&event);
            match serde_json::to_string(&event) {
                Ok(json) => Some(Ok(Event::default().event(event_type).data(json))),
                Err(_) => None,
            }
        }
        Err(_) => Some(Ok(Event::default().event("resync").data("{}"))),
    });
    let stream = initial_stream.chain(live_stream);

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dashboard_events::DashboardEvent,
        state::{AppState, CachedUpsState},
    };
    use sqlx::{PgPool, postgres::PgPoolOptions};

    fn make_lazy_pool() -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://postgres:postgres@127.0.0.1:5432/postgres")
            .expect("lazy pool")
    }

    #[tokio::test]
    async fn cached_dashboard_events_include_forwarder_ups_state() {
        let state = AppState::new(make_lazy_pool());
        state.forwarder_ups_cache.write().await.insert(
            "fwd-1".to_owned(),
            CachedUpsState {
                available: false,
                status: Some(rt_protocol::UpsStatus {
                    battery_percent: 33,
                    battery_voltage_mv: 3810,
                    charging: false,
                    power_plugged: false,
                    temperature_cdeg: 2750,
                    sampled_at: 1711929600000,
                }),
            },
        );

        let events = cached_dashboard_events(&state).await;

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            DashboardEvent::ForwarderUpsUpdated {
                forwarder_id,
                available: false,
                status: Some(status),
            } if forwarder_id == "fwd-1" && status.battery_percent == 33
        ));
    }
}
