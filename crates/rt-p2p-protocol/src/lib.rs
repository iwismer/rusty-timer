//! Clean-slate P2P wire protocol for the forwarder <-> receiver iroh transport.
//!
//! The wire format is protobuf, encoded/decoded via [`prost`]. The protocol is
//! split into a **control plane** and a **data plane**, each with directional
//! envelope messages:
//!
//! - [`ControlC2F`] / [`ControlF2C`] — control plane (client/receiver <-> forwarder)
//! - [`DataC2F`] / [`DataF2C`] — data plane (client/receiver <-> forwarder)
//!
//! where `C2F` is client (receiver) -> forwarder and `F2C` is forwarder ->
//! client (receiver).
//!
//! # Hermetic codegen
//!
//! The generated Rust in `src/generated/` is **checked in**, so a normal
//! `cargo build` (and CI / arm64 cross-builds) does **not** require `protoc`.
//! The [`build.rs`](../build.rs) script is a no-op sanity check only.
//!
//! ## Regenerating after editing `proto/p2p.proto`
//!
//! Regeneration is an explicit, opt-in step driven by a small throwaway
//! `prost-build` program that supplies a vendored `protoc` (so no host `protoc`
//! is needed). See the crate `README.md` for the exact driver `Cargo.toml` and
//! `src/main.rs`; it overwrites `src/generated/rusty_timer.p2p.v1.rs`.
//!
//! After regenerating, review the diff, run `cargo fmt`, and commit the updated
//! `src/generated/rusty_timer.p2p.v1.rs`. The build script never performs this
//! step.

pub mod capabilities;
pub mod codec;
pub mod error;
pub mod negotiate;

// The generated module mirrors `protoc`/`prost-build` output and intentionally
// does not follow this workspace's stricter lints.
#[allow(clippy::all, clippy::pedantic, missing_docs)]
mod generated {
    include!("generated/rusty_timer.p2p.v1.rs");
}

/// Generated protobuf messages and oneof modules.
pub mod proto {
    pub use crate::generated::*;
}

pub use capabilities::{CAP_CONTROL_EVENTS, CAP_REMOTE_CONFIG, has_capability};
pub use codec::{Frame, MAX_FRAME_BYTES, decode_frame, decode_message_frame, encode_frame};
pub use error::{ProtocolError, ProtocolErrorCode};
pub use generated::{
    Ack, CaughtUp, ControlC2F, ControlF2C, DataC2F, DataF2C, DataSubscribe, DownloadProgress,
    EventBatch, GapNotice, Hello, HelloOk, Ping, Pong, ProtocolError as WireProtocolError,
    ReadRecord, ReaderControlRequest, ReaderControlResponse, ReaderInfo, ReaderStatus,
    StreamCatalog, StreamEntry, StreamEpochStarted, SubscribeMode, SubscribeOk, SyncClock,
    UpsStatus, control_c2f, control_f2c, data_c2f, data_f2c,
};
pub use negotiate::negotiate;
