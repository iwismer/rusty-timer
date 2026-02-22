pub mod events;
pub mod forwarder_races;
pub mod races;
pub mod reads;
pub mod receiver_cursors;
pub mod stream_epoch_races;

pub struct EventRow {
    pub stream_epoch: i64,
    pub seq: i64,
    pub reader_timestamp: Option<String>,
    pub raw_read_line: String,
    pub read_type: String,
    pub forwarder_id: String,
    pub reader_ip: String,
}
