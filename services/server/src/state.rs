use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

pub type StreamBroadcast = broadcast::Sender<rt_protocol::ReadEvent>;
pub type BroadcastRegistry = Arc<RwLock<HashMap<Uuid, StreamBroadcast>>>;
pub type ForwarderCommandSenders =
    Arc<RwLock<HashMap<String, tokio::sync::mpsc::Sender<rt_protocol::EpochResetCommand>>>>;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub active_forwarders: Arc<RwLock<HashMap<String, ()>>>,
    pub broadcast_registry: BroadcastRegistry,
    pub forwarder_command_senders: ForwarderCommandSenders,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            active_forwarders: Arc::new(RwLock::new(HashMap::new())),
            broadcast_registry: Arc::new(RwLock::new(HashMap::new())),
            forwarder_command_senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_forwarder(&self, device_id: &str) -> bool {
        let mut map = self.active_forwarders.write().await;
        if map.contains_key(device_id) { false } else { map.insert(device_id.to_owned(), ()); true }
    }

    pub async fn unregister_forwarder(&self, device_id: &str) {
        self.active_forwarders.write().await.remove(device_id);
    }

    pub async fn get_or_create_broadcast(&self, stream_id: Uuid) -> StreamBroadcast {
        {
            let reg = self.broadcast_registry.read().await;
            if let Some(tx) = reg.get(&stream_id) { return tx.clone(); }
        }
        let mut reg = self.broadcast_registry.write().await;
        if let Some(tx) = reg.get(&stream_id) { return tx.clone(); }
        let (tx, _rx) = broadcast::channel(1024);
        reg.insert(stream_id, tx.clone());
        tx
    }
}
