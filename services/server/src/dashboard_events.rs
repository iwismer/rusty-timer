use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DashboardEvent {
    StreamCreated {
        stream_id: Uuid,
        forwarder_id: String,
        reader_ip: String,
        display_alias: Option<String>,
        online: bool,
        stream_epoch: i64,
        created_at: String,
    },
    StreamUpdated {
        stream_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        online: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stream_epoch: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        display_alias: Option<String>,
    },
    MetricsUpdated {
        stream_id: Uuid,
        raw_count: i64,
        dedup_count: i64,
        retransmit_count: i64,
        lag_ms: Option<u64>,
    },
}
