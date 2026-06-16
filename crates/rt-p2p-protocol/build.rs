//! Build script for `rt-p2p-protocol`.
//!
//! This crate uses **checked-in** generated Rust (see `src/generated/`), so a
//! normal `cargo build` does **not** require `protoc` on the host. This keeps
//! CI and arm64 cross-builds hermetic.
//!
//! The build script is intentionally a no-op aside from a cheap sanity check
//! that the proto source is present and instructing Cargo when to re-run.
//!
//! To regenerate the checked-in Rust after editing `proto/p2p.proto`, run the
//! explicit, opt-in `prost-build` driver documented in `README.md`. It vendors
//! its own `protoc`, so no host `protoc` (and no codegen dependency in this
//! crate) is needed for a normal build.

use std::path::Path;

fn main() {
    let proto = Path::new("proto/p2p.proto");

    // Sanity check: the canonical proto definition must exist. This catches an
    // accidental deletion without pulling in any codegen dependency.
    assert!(
        proto.exists(),
        "missing proto/p2p.proto; the protocol definition is required"
    );

    // Re-run only when the proto changes; the generated Rust is checked in and
    // is not produced by this build script.
    println!("cargo:rerun-if-changed=proto/p2p.proto");
    println!("cargo:rerun-if-changed=build.rs");
}
