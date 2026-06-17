//! Production forwarder peer-to-peer (P2P) transport.
//!
//! This module owns the forwarder's [`rt_iroh`] endpoint and the accept loop
//! that admits inbound receiver connections. Admission is gated by a persistent
//! [`AllowList`] keyed on the remote peer's iroh node id (the transport-layer
//! `EndpointId`): connections from peers that are not on the allow-list are
//! closed before any control-plane work happens. The allow-list caches its set
//! on disk (fail-to-last-known on refresh failures) and force-closes a peer's
//! open connections when an update revokes it.
//!
//! Scope: production startup currently wires the endpoint, accept loop, and
//! allow-listed control-plane handshake. The data-stream subscriber handler and
//! the thin-node allow-list distribution components ([`ThinNodeAllowListClient`]
//! and [`run_allowlist_distribution`]) are available for P2P wiring, while the
//! reader control/status mapping remains a later task.

mod allowlist;
mod control;
mod data;
mod endpoint;

pub use allowlist::{
    AllowList, AllowListRefreshError, DEFAULT_ALLOWLIST_POLL_INTERVAL, ReceiverAllowListUpdate,
    ThinNodeAllowListClient, apply_receiver_update, fetch_and_apply_once,
    run_allowlist_distribution,
};
pub use control::{
    CatalogProvider, ControlEvent, ControlEventReceiver, ControlEventSender, HeartbeatConfig,
    NoopReaderControlHandler, ReaderControlFuture, ReaderControlHandler, RewriteClockFuture,
    StaticCatalog, SyncClockDriftHandler, SyncClockFuture, SyncClockSource, control_event_channel,
};
pub use data::{DataConfig, serve_data_streams};
pub use endpoint::P2pEndpoint;
