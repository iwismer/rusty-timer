use serde::{Serialize, Serializer};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum OptionalStringPatch {
    Set(String),
    Clear,
}

impl Serialize for OptionalStringPatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Set(value) => serializer.serialize_str(value),
            Self::Clear => serializer.serialize_none(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DashboardEvent {
    StreamCreated {
        stream_id: Uuid,
        forwarder_id: String,
        reader_ip: String,
        display_alias: Option<String>,
        forwarder_display_name: Option<String>,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        forwarder_display_name: Option<OptionalStringPatch>,
    },
    MetricsUpdated {
        stream_id: Uuid,
        raw_count: i64,
        dedup_count: i64,
        retransmit_count: i64,
        lag_ms: Option<u64>,
        epoch_raw_count: i64,
        epoch_dedup_count: i64,
        epoch_retransmit_count: i64,
        epoch_lag_ms: Option<u64>,
        epoch_last_received_at: Option<String>,
        unique_chips: i64,
        last_tag_id: Option<String>,
        last_reader_timestamp: Option<String>,
    },
    ForwarderRaceAssigned {
        forwarder_id: String,
        race_id: Option<Uuid>,
    },
}
