//! Receiver control API handlers, split by domain.
//!
//! Shared state ([`crate::control_api::AppState`]) and shared types live in
//! `control_api.rs`, which re-exports every item here so external callers keep
//! using `control_api::*` paths.

pub mod forwarders;
pub mod imports;
pub mod profile;
pub mod status;
pub mod subscriptions;
