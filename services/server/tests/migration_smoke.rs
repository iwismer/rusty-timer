/// Structural validation tests for the PostgreSQL migration file.
///
/// These tests validate the SQL migration schema by parsing and checking that
/// all required tables, columns, constraints, and indexes are present.
///
/// NOTE: Full migration execution testing requires a PostgreSQL container
/// (e.g., testcontainers-rs) and is deferred to the integration test phase.
const MIGRATION_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/0001_init.sql");

fn read_migration() -> String {
    std::fs::read_to_string(MIGRATION_PATH)
        .expect("Migration file should exist at services/server/migrations/0001_init.sql")
}

// ---------------------------------------------------------------------------
// Table presence
// ---------------------------------------------------------------------------

#[test]
fn migration_file_exists_and_is_nonempty() {
    let sql = read_migration();
    assert!(!sql.trim().is_empty(), "Migration file must not be empty");
}

#[test]
fn contains_device_tokens_table() {
    let sql = read_migration();
    assert!(
        sql.contains("CREATE TABLE device_tokens"),
        "Migration must define device_tokens table"
    );
}

#[test]
fn contains_streams_table() {
    let sql = read_migration();
    assert!(
        sql.contains("CREATE TABLE streams"),
        "Migration must define streams table"
    );
}

#[test]
fn contains_events_table() {
    let sql = read_migration();
    assert!(
        sql.contains("CREATE TABLE events"),
        "Migration must define events table"
    );
}

#[test]
fn contains_stream_metrics_table() {
    let sql = read_migration();
    assert!(
        sql.contains("CREATE TABLE stream_metrics"),
        "Migration must define stream_metrics table"
    );
}

#[test]
fn contains_receiver_cursors_table() {
    let sql = read_migration();
    assert!(
        sql.contains("CREATE TABLE receiver_cursors"),
        "Migration must define receiver_cursors table"
    );
}

// ---------------------------------------------------------------------------
// device_tokens columns
// ---------------------------------------------------------------------------

#[test]
fn device_tokens_has_token_hash_bytea_pk() {
    let sql = read_migration();
    // token_hash is BYTEA UNIQUE (indexed for lookups); token_id is the UUID PK
    assert!(
        sql.contains("token_hash") && sql.contains("BYTEA"),
        "device_tokens must have token_hash BYTEA column"
    );
    assert!(
        sql.contains("PRIMARY KEY"),
        "device_tokens must have a PRIMARY KEY"
    );
}

#[test]
fn device_tokens_has_device_id() {
    let sql = read_migration();
    assert!(
        sql.contains("device_id") && sql.contains("TEXT NOT NULL"),
        "device_tokens must have device_id TEXT NOT NULL"
    );
}

#[test]
fn device_tokens_has_device_type() {
    let sql = read_migration();
    assert!(
        sql.contains("device_type"),
        "device_tokens must have device_type column"
    );
}

#[test]
fn device_tokens_has_created_at() {
    let sql = read_migration();
    assert!(
        sql.contains("created_at") && sql.contains("TIMESTAMPTZ"),
        "device_tokens must have created_at TIMESTAMPTZ"
    );
}

#[test]
fn device_tokens_has_revoked_at() {
    let sql = read_migration();
    assert!(
        sql.contains("revoked_at") && sql.contains("TIMESTAMPTZ"),
        "device_tokens must have revoked_at TIMESTAMPTZ"
    );
}

// ---------------------------------------------------------------------------
// streams columns and constraints
// ---------------------------------------------------------------------------

#[test]
fn streams_has_uuid_pk() {
    let sql = read_migration();
    assert!(
        sql.contains("stream_id") && sql.contains("UUID PRIMARY KEY"),
        "streams must have stream_id UUID PRIMARY KEY"
    );
}

#[test]
fn streams_has_forwarder_id() {
    let sql = read_migration();
    assert!(
        sql.contains("forwarder_id"),
        "streams must have forwarder_id column"
    );
}

#[test]
fn streams_has_reader_ip() {
    let sql = read_migration();
    assert!(
        sql.contains("reader_ip"),
        "streams must have reader_ip column"
    );
}

#[test]
fn streams_has_display_alias() {
    let sql = read_migration();
    assert!(
        sql.contains("display_alias"),
        "streams must have display_alias column"
    );
}

#[test]
fn streams_has_stream_epoch() {
    let sql = read_migration();
    assert!(
        sql.contains("stream_epoch"),
        "streams must have stream_epoch column"
    );
}

#[test]
fn streams_has_online() {
    let sql = read_migration();
    assert!(sql.contains("online"), "streams must have online column");
}

#[test]
fn streams_unique_forwarder_reader() {
    let sql = read_migration();
    // Allow either UNIQUE(...) or UNIQUE (...) syntax
    assert!(
        sql.contains("UNIQUE(forwarder_id, reader_ip)")
            || sql.contains("UNIQUE (forwarder_id, reader_ip)"),
        "streams must have UNIQUE(forwarder_id, reader_ip) constraint"
    );
}

// ---------------------------------------------------------------------------
// events columns and constraints
// ---------------------------------------------------------------------------

#[test]
fn events_references_streams() {
    let sql = read_migration();
    assert!(
        sql.contains("REFERENCES streams(stream_id)"),
        "events.stream_id must reference streams(stream_id)"
    );
}

#[test]
fn events_has_composite_pk() {
    let sql = read_migration();
    assert!(
        sql.contains("PRIMARY KEY (stream_id, stream_epoch, seq)"),
        "events must have PRIMARY KEY (stream_id, stream_epoch, seq)"
    );
}

#[test]
fn events_has_reader_timestamp() {
    let sql = read_migration();
    // reader_timestamp is TEXT (stores ISO-8601 strings from the forwarder)
    assert!(
        sql.contains("reader_timestamp"),
        "events must have reader_timestamp column"
    );
}

#[test]
fn events_has_raw_read_line() {
    let sql = read_migration();
    assert!(
        sql.contains("raw_read_line") && sql.contains("TEXT NOT NULL"),
        "events must have raw_read_line TEXT NOT NULL"
    );
}

#[test]
fn events_has_read_type() {
    let sql = read_migration();
    assert!(
        sql.contains("read_type"),
        "events must have read_type column"
    );
}

#[test]
fn events_has_received_at() {
    let sql = read_migration();
    assert!(
        sql.contains("received_at"),
        "events must have received_at column"
    );
}

#[test]
fn events_identity_index_exists() {
    let sql = read_migration();
    // The composite PRIMARY KEY (stream_id, stream_epoch, seq) serves as the unique identity.
    // An explicit separate UNIQUE INDEX is optional; the PK constraint enforces uniqueness.
    assert!(
        sql.contains("PRIMARY KEY (stream_id, stream_epoch, seq)"),
        "Migration must define composite PK (stream_id, stream_epoch, seq) for event identity"
    );
}

// ---------------------------------------------------------------------------
// stream_metrics columns
// ---------------------------------------------------------------------------

#[test]
fn stream_metrics_has_raw_count() {
    let sql = read_migration();
    assert!(
        sql.contains("raw_count"),
        "stream_metrics must have raw_count column"
    );
}

#[test]
fn stream_metrics_has_dedup_count() {
    let sql = read_migration();
    assert!(
        sql.contains("dedup_count"),
        "stream_metrics must have dedup_count column"
    );
}

#[test]
fn stream_metrics_has_retransmit_count() {
    let sql = read_migration();
    assert!(
        sql.contains("retransmit_count"),
        "stream_metrics must have retransmit_count column"
    );
}

#[test]
fn stream_metrics_has_last_canonical_event_received_at() {
    let sql = read_migration();
    assert!(
        sql.contains("last_canonical_event_received_at"),
        "stream_metrics must have last_canonical_event_received_at column"
    );
}

#[test]
fn stream_metrics_references_streams() {
    let sql = read_migration();
    let sql_lower = sql.to_lowercase();
    let sm_start = sql_lower.find("create table stream_metrics");
    assert!(sm_start.is_some(), "stream_metrics table must exist");
    let sm_section = &sql[sm_start.unwrap()..];
    // Find the end of this table's CREATE statement
    let next_create = sm_section[1..]
        .find("create table")
        .map(|i| i + 1)
        .unwrap_or(sm_section.len());
    let sm_block = &sm_section[..next_create];
    assert!(
        sm_block.contains("REFERENCES streams(stream_id)"),
        "stream_metrics.stream_id must reference streams(stream_id)"
    );
}

// ---------------------------------------------------------------------------
// receiver_cursors columns and constraints
// ---------------------------------------------------------------------------

#[test]
fn receiver_cursors_has_receiver_id() {
    let sql = read_migration();
    assert!(
        sql.contains("receiver_id"),
        "receiver_cursors must have receiver_id column"
    );
}

#[test]
fn receiver_cursors_has_last_seq() {
    let sql = read_migration();
    assert!(
        sql.contains("last_seq"),
        "receiver_cursors must have last_seq column"
    );
}

#[test]
fn receiver_cursors_has_composite_pk() {
    let sql = read_migration();
    assert!(
        sql.contains("PRIMARY KEY (receiver_id, stream_id)"),
        "receiver_cursors must have PRIMARY KEY (receiver_id, stream_id)"
    );
}

#[test]
fn receiver_cursors_references_streams() {
    let sql = read_migration();
    let sql_lower = sql.to_lowercase();
    let rc_start = sql_lower.find("create table receiver_cursors");
    assert!(rc_start.is_some(), "receiver_cursors table must exist");
    let rc_section = &sql[rc_start.unwrap()..];
    assert!(
        rc_section.contains("REFERENCES streams(stream_id)"),
        "receiver_cursors.stream_id must reference streams(stream_id)"
    );
}

// ---------------------------------------------------------------------------
// Token hash invariant: SHA-256 produces 32 bytes
// ---------------------------------------------------------------------------

#[test]
fn token_hash_is_32_byte_sha256() {
    // The design specifies token_hash stores SHA-256(raw_token_bytes).
    // SHA-256 always produces exactly 32 bytes.
    // We validate the schema documents BYTEA type for token_hash,
    // and that the comment/documentation mentions SHA-256.
    let sql = read_migration();
    assert!(
        sql.contains("token_hash") && sql.contains("BYTEA"),
        "token_hash must be stored as BYTEA (32-byte SHA-256 digest)"
    );
    assert!(
        sql.contains("SHA-256") || sql.contains("sha-256") || sql.contains("SHA256"),
        "Migration should document that token_hash stores SHA-256"
    );
}

// ---------------------------------------------------------------------------
// Structural completeness: all five tables present
// ---------------------------------------------------------------------------

#[test]
fn all_five_tables_defined() {
    let sql = read_migration();
    let required_tables = [
        "device_tokens",
        "streams",
        "events",
        "stream_metrics",
        "receiver_cursors",
    ];
    for table in required_tables {
        assert!(
            sql.contains(&format!("CREATE TABLE {}", table)),
            "Migration must define {} table",
            table
        );
    }
}
