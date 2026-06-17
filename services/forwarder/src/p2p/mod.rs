//! Production forwarder peer-to-peer (P2P) transport.
//!
//! This module owns the forwarder's [`rt_iroh`] endpoint and the accept loop
//! that admits inbound receiver connections. Admission is gated by an in-memory
//! allow-list keyed on the remote peer's iroh node id (the transport-layer
//! `EndpointId`): connections from peers that are not on the allow-list are
//! closed before any control-plane work happens.
//!
//! Scope: this module is intentionally limited to the endpoint, the accept
//! loop, and allow-listed handshake admission. The persistent allow-list,
//! revocation, the full control-plane catalog/heartbeat, and data-plane
//! delivery are implemented in later tasks.

mod endpoint;

pub use endpoint::{AllowList, P2pEndpoint};
