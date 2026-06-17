//! Production forwarder peer-to-peer (P2P) transport.
//!
//! This module owns the forwarder's [`rt_iroh`] endpoint and the accept loop
//! that admits inbound receiver connections. Admission is gated by an in-memory
//! allow-list keyed on the remote peer's iroh node id (the transport-layer
//! `EndpointId`): connections from peers that are not on the allow-list are
//! closed before any control-plane work happens.
//!
//! Scope: production startup currently wires the endpoint, accept loop, and
//! allow-listed control-plane handshake. The data-stream subscriber handler is
//! available for P2P wiring, while persistent allow-list/revocation and reader
//! control/status mapping remain later tasks.

mod control;
mod data;
mod endpoint;

pub use control::{CatalogProvider, HeartbeatConfig, StaticCatalog};
pub use data::{DataConfig, serve_data_streams};
pub use endpoint::{AllowList, P2pEndpoint};
