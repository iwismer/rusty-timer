use rt_protocol::{ConfigGetResponse, ConfigSetResponse, EpochResetCommand, RestartResponse};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot, RwLock};
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

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub active_forwarders: Arc<RwLock<HashMap<String, ()>>>,
    pub broadcast_registry: BroadcastRegistry,
    pub forwarder_command_senders: ForwarderCommandSenders,
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
}
