// forwarder: Library entry point.
// Exposes modules for integration testing.

use std::path::PathBuf;

pub mod config;
pub mod discovery;
pub mod local_fanout;
pub mod replay;
pub mod status_http;
pub mod storage;
pub mod ui_events;
pub mod ui_server;
pub mod uplink;
pub mod uplink_replay;

pub const DEFAULT_UPDATER_STAGE_DIR: &str = "/var/lib/rusty-timer";

#[must_use]
pub fn updater_stage_root_dir() -> PathBuf {
    match std::env::var_os("RT_UPDATER_STAGE_DIR") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(DEFAULT_UPDATER_STAGE_DIR),
    }
}
