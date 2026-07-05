//! Loopback iroh peer harness for forwarder <-> receiver P2P tests.
//!
//! This module provides deterministic, test-only peers that speak the
//! [`rt_p2p_protocol`] wire format over a loopback [`rt_iroh`] endpoint:
//!
//! - [`MockForwarderPeer`] serves a scripted [`StreamCatalog`](rt_p2p_protocol::StreamCatalog)
//!   plus scripted [`EventBatch`](rt_p2p_protocol::EventBatch) frames.
//! - [`MockReceiverPeer`] dials a forwarder, performs the `Hello` negotiation,
//!   subscribes to a stream, collects batches, and sends acknowledgements.
//!
//! Endpoints are built with [`rt_iroh::EndpointBuilder::test`], which seeds the
//! secret key, disables relays, clears discovery, and binds loopback-only — so
//! the harness is hermetic and deterministic.
//!
//! ## Plane convention
//!
//! Each connection carries one **control** bidirectional stream followed by one
//! or more **data** bidirectional streams. The receiver opens the control
//! stream first (sending `Hello`) and a data stream per subscription (sending
//! `DataSubscribe`); the forwarder accepts them in that order.

mod mock_forwarder;
mod mock_receiver;

use std::time::Duration;

use prost::Message;
use rt_iroh::{RecvStream, SendStream};
use rt_p2p_protocol::{decode_frame_len, decode_frame_payload, encode_frame};

pub use mock_forwarder::{BatchGate, ForwarderScript, MockForwarderPeer};
pub use mock_receiver::{DataSubscription, MockReceiverPeer, ReceiverSession};

/// Result type used throughout the harness.
pub type HarnessResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Writes a single length-prefixed protobuf frame to a send stream.
pub(crate) async fn write_frame(send: &mut SendStream, message: &impl Message) -> HarnessResult {
    send.write_all(&encode_frame(message)).await?;
    Ok(())
}

/// Reads a single length-prefixed protobuf frame from a receive stream.
pub(crate) async fn read_frame<M>(recv: &mut RecvStream) -> HarnessResult<M>
where
    M: Message + Default,
{
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = decode_frame_len(len_buf)?;
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload).await?;
    Ok(decode_frame_payload(len_buf, payload.as_slice())?)
}

/// Test-only connectivity fault shim.
///
/// A simple, deterministic description of degraded connectivity for harness
/// tests (drop, delay, partition). [`ForwarderScript`](crate::p2p::ForwarderScript)
/// consults this before data-plane frame writes and inbound ack reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectivityFault {
    /// Drop outbound frames instead of sending them.
    pub drop_outbound: bool,
    /// Inject an artificial delay before each outbound frame.
    pub extra_delay: Option<Duration>,
    /// Treat the link as fully partitioned (no traffic in either direction).
    pub partitioned: bool,
}

impl ConnectivityFault {
    /// A fault-free link.
    pub fn healthy() -> Self {
        Self::default()
    }

    /// A link that silently drops outbound frames.
    pub fn dropping() -> Self {
        Self {
            drop_outbound: true,
            ..Self::default()
        }
    }

    /// A link that delays every outbound frame by `delay`.
    pub fn delayed(delay: Duration) -> Self {
        Self {
            extra_delay: Some(delay),
            ..Self::default()
        }
    }

    /// A fully partitioned link.
    pub fn partitioned() -> Self {
        Self {
            partitioned: true,
            ..Self::default()
        }
    }

    /// Whether a frame should be dropped under this fault.
    pub fn should_drop(&self) -> bool {
        self.drop_outbound || self.partitioned
    }

    /// Applies the configured delay, if any.
    pub async fn apply_delay(&self) {
        if let Some(delay) = self.extra_delay {
            tokio::time::sleep(delay).await;
        }
    }
}
