use rt_protocol::{ConfigGetResponse, ConfigSetResponse, EpochResetCommand, RestartResponse};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, oneshot};
use uuid::Uuid;

use crate::dashboard_events::DashboardEvent;

pub enum ForwarderProxyReply<T> {
    Response(T),
    Timeout,
}

pub enum ForwarderCommand {
    EpochReset(EpochResetCommand),
    ConfigGet {
        request_id: String,
        reply: oneshot::Sender<ForwarderProxyReply<ConfigGetResponse>>,
    },
    ConfigSet {
        request_id: String,
        section: String,
        payload: serde_json::Value,
        reply: oneshot::Sender<ForwarderProxyReply<ConfigSetResponse>>,
    },
    Restart {
        request_id: String,
        reply: oneshot::Sender<ForwarderProxyReply<RestartResponse>>,
    },
}

pub type StreamBroadcast = broadcast::Sender<rt_protocol::ReadEvent>;
pub type BroadcastRegistry = Arc<RwLock<HashMap<Uuid, StreamBroadcast>>>;
pub type ForwarderCommandSenders =
    Arc<RwLock<HashMap<String, tokio::sync::mpsc::Sender<ForwarderCommand>>>>;
pub type ReceiverSessionRegistry = Arc<RwLock<HashMap<String, ReceiverSessionRecord>>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiverSessionProtocol {
    V12,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiverSelectionSnapshot {
    /// v1.2 mode-based snapshot.
    Mode {
        mode_summary: String,
        race_id: Option<Uuid>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiverSessionRecord {
    pub receiver_id: String,
    pub protocol: ReceiverSessionProtocol,
    pub selection: ReceiverSelectionSnapshot,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub active_forwarders: Arc<RwLock<HashMap<String, ()>>>,
    pub broadcast_registry: BroadcastRegistry,
    pub forwarder_command_senders: ForwarderCommandSenders,
    pub active_receiver_sessions: ReceiverSessionRegistry,
    pub dashboard_tx: broadcast::Sender<DashboardEvent>,
    pub logger: Arc<rt_ui_log::UiLogger<DashboardEvent>>,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        let (dashboard_tx, _) = broadcast::channel(4096);
        let logger = Arc::new(rt_ui_log::UiLogger::with_buffer(
            dashboard_tx.clone(),
            |entry| DashboardEvent::LogEntry { entry },
            500,
        ));
        Self {
            pool,
            active_forwarders: Arc::new(RwLock::new(HashMap::new())),
            broadcast_registry: Arc::new(RwLock::new(HashMap::new())),
            forwarder_command_senders: Arc::new(RwLock::new(HashMap::new())),
            active_receiver_sessions: Arc::new(RwLock::new(HashMap::new())),
            dashboard_tx,
            logger,
        }
    }

    pub async fn register_forwarder(&self, device_id: &str) -> bool {
        let mut map = self.active_forwarders.write().await;
        if map.contains_key(device_id) {
            false
        } else {
            map.insert(device_id.to_owned(), ());
            true
        }
    }

    pub async fn unregister_forwarder(&self, device_id: &str) {
        self.active_forwarders.write().await.remove(device_id);
    }

    pub async fn get_or_create_broadcast(&self, stream_id: Uuid) -> StreamBroadcast {
        {
            let reg = self.broadcast_registry.read().await;
            if let Some(tx) = reg.get(&stream_id) {
                return tx.clone();
            }
        }
        let mut reg = self.broadcast_registry.write().await;
        if let Some(tx) = reg.get(&stream_id) {
            return tx.clone();
        }
        let (tx, _rx) = broadcast::channel(1024);
        reg.insert(stream_id, tx.clone());
        tx
    }

    pub async fn register_receiver_session(
        &self,
        session_id: &str,
        receiver_id: &str,
        protocol: ReceiverSessionProtocol,
        selection: ReceiverSelectionSnapshot,
    ) {
        self.active_receiver_sessions.write().await.insert(
            session_id.to_owned(),
            ReceiverSessionRecord {
                receiver_id: receiver_id.to_owned(),
                protocol,
                selection,
            },
        );
    }

    pub async fn update_receiver_session_selection(
        &self,
        session_id: &str,
        selection: ReceiverSelectionSnapshot,
    ) -> bool {
        if let Some(record) = self
            .active_receiver_sessions
            .write()
            .await
            .get_mut(session_id)
        {
            record.selection = selection;
            return true;
        }
        false
    }

    pub async fn unregister_receiver_session(&self, session_id: &str) {
        self.active_receiver_sessions
            .write()
            .await
            .remove(session_id);
    }

    pub async fn get_receiver_session(&self, session_id: &str) -> Option<ReceiverSessionRecord> {
        self.active_receiver_sessions
            .read()
            .await
            .get(session_id)
            .cloned()
    }

    pub async fn has_active_receiver_session_for_race(&self, race_id: Uuid) -> bool {
        self.active_receiver_sessions
            .read()
            .await
            .values()
            .any(|record| match &record.selection {
                ReceiverSelectionSnapshot::Mode {
                    race_id: Some(selected_race_id),
                    ..
                } => *selected_race_id == race_id,
                ReceiverSelectionSnapshot::Mode { race_id: None, .. } => false,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    fn make_lazy_pool() -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://postgres:postgres@127.0.0.1:5432/postgres")
            .expect("lazy pool")
    }

    #[tokio::test]
    async fn receiver_session_registry_tracks_register_update_and_unregister() {
        let state = AppState::new(make_lazy_pool());
        let session_id = Uuid::new_v4().to_string();

        state
            .register_receiver_session(
                &session_id,
                "receiver-1",
                ReceiverSessionProtocol::V12,
                ReceiverSelectionSnapshot::Mode {
                    mode_summary: "race (race-1)".to_owned(),
                    race_id: None,
                },
            )
            .await;

        let record = state
            .get_receiver_session(&session_id)
            .await
            .expect("session should exist");
        assert_eq!(record.receiver_id, "receiver-1");
        assert_eq!(record.protocol, ReceiverSessionProtocol::V12);
        assert_eq!(
            record.selection,
            ReceiverSelectionSnapshot::Mode {
                mode_summary: "race (race-1)".to_owned(),
                race_id: None,
            }
        );

        let updated = state
            .update_receiver_session_selection(
                &session_id,
                ReceiverSelectionSnapshot::Mode {
                    mode_summary: "live (1 streams)".to_owned(),
                    race_id: None,
                },
            )
            .await;
        assert!(updated);

        let record = state
            .get_receiver_session(&session_id)
            .await
            .expect("session should still exist");
        assert_eq!(
            record.selection,
            ReceiverSelectionSnapshot::Mode {
                mode_summary: "live (1 streams)".to_owned(),
                race_id: None,
            }
        );

        state.unregister_receiver_session(&session_id).await;
        assert!(state.get_receiver_session(&session_id).await.is_none());
    }

    #[tokio::test]
    async fn receiver_session_registry_can_query_active_race_selection_by_race_id() {
        let state = AppState::new(make_lazy_pool());
        let selected_race_id = Uuid::new_v4();
        let other_race_id = Uuid::new_v4();

        state
            .register_receiver_session(
                "session-race",
                "receiver-race",
                ReceiverSessionProtocol::V12,
                ReceiverSelectionSnapshot::Mode {
                    mode_summary: format!("race ({selected_race_id})"),
                    race_id: Some(selected_race_id),
                },
            )
            .await;

        state
            .register_receiver_session(
                "session-manual",
                "receiver-manual",
                ReceiverSessionProtocol::V12,
                ReceiverSelectionSnapshot::Mode {
                    mode_summary: "live (1 streams)".to_owned(),
                    race_id: None,
                },
            )
            .await;

        assert!(
            state
                .has_active_receiver_session_for_race(selected_race_id)
                .await
        );
        assert!(
            !state
                .has_active_receiver_session_for_race(other_race_id)
                .await
        );

        state.unregister_receiver_session("session-race").await;
        assert!(
            !state
                .has_active_receiver_session_for_race(selected_race_id)
                .await
        );
    }

    #[tokio::test]
    async fn receiver_session_registry_matches_equivalent_uuid_text_and_ignores_invalid_ids() {
        let state = AppState::new(make_lazy_pool());
        let selected_race_id = Uuid::new_v4();

        state
            .register_receiver_session(
                "session-race-uppercase",
                "receiver-race",
                ReceiverSessionProtocol::V12,
                ReceiverSelectionSnapshot::Mode {
                    mode_summary: format!("race ({})", selected_race_id.to_string().to_uppercase()),
                    race_id: Some(selected_race_id),
                },
            )
            .await;

        state
            .register_receiver_session(
                "session-race-invalid",
                "receiver-race",
                ReceiverSessionProtocol::V12,
                ReceiverSelectionSnapshot::Mode {
                    mode_summary: "race (not-a-uuid)".to_owned(),
                    race_id: None,
                },
            )
            .await;

        assert!(
            state
                .has_active_receiver_session_for_race(selected_race_id)
                .await
        );
    }

    #[tokio::test]
    async fn active_race_query_uses_structured_race_id_not_summary_text() {
        let state = AppState::new(make_lazy_pool());
        let race_id = Uuid::new_v4();

        state
            .register_receiver_session(
                "session-race-structured",
                "receiver-race",
                ReceiverSessionProtocol::V12,
                ReceiverSelectionSnapshot::Mode {
                    mode_summary: "non-standard race summary".to_owned(),
                    race_id: Some(race_id),
                },
            )
            .await;

        assert!(state.has_active_receiver_session_for_race(race_id).await);
    }
}
