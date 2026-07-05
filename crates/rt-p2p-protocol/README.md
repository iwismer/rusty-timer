# rt-p2p-protocol

Clean-slate P2P wire protocol for the forwarder ↔ receiver iroh transport.

The wire format is protobuf (`proto/p2p.proto`), encoded/decoded with
[`prost`]. It is split into a control plane and a data plane, each with
directional envelope messages:

- `ControlC2F` / `ControlF2C` — control plane
- `DataC2F` / `DataF2C` — data plane

where `C2F` = client (receiver) → forwarder and `F2C` = forwarder → client
(receiver).

## Hermetic builds (no `protoc` required)

The prost-generated Rust is **checked in** at
`src/generated/rusty_timer.p2p.v1.rs`. A normal `cargo build` — including CI and
arm64 cross-builds — does **not** need `protoc`. The `build.rs` is a no-op
sanity check that only verifies `proto/p2p.proto` exists and sets up
`rerun-if-changed`.

## Regenerating the checked-in Rust

Regeneration is explicit and opt-in. The checked-in file is produced by
`prost-build`; reproduce it with a small throwaway driver rather than relying on
a host `protoc` or an implicitly-installed `protoc-gen-prost` plugin.

Create a scratch crate (e.g. under `/tmp`) with this `Cargo.toml`:

```toml
[package]
name = "regen-p2p"
version = "0.0.0"
edition = "2021"

[dependencies]
prost-build = "0.14"
protoc-bin-vendored = "3"
```

and this `src/main.rs`:

```rust
fn main() {
    // Use a vendored protoc so no host `protoc` is required.
    // SAFETY: single-threaded build driver, set before any codegen runs.
    unsafe {
        std::env::set_var(
            "PROTOC",
            protoc_bin_vendored::protoc_bin_path().unwrap(),
        );
    }
    let mut cfg = prost_build::Config::new();
    cfg.out_dir("src/generated");
    cfg.compile_protos(&["proto/p2p.proto"], &["proto"]).unwrap();
}
```

Run it from this crate directory (paths are relative to the crate root):

```sh
cargo run --manifest-path /tmp/regen-p2p/Cargo.toml
```

It overwrites `src/generated/rusty_timer.p2p.v1.rs`. Then review the diff, run
`cargo fmt`, and commit the regenerated file together with the proto change. The
normal build never performs this step and never needs `protoc`.

Note: prost-build 0.14 emits the envelope structs as `ControlC2f`/`ControlF2c`/
`DataC2f`/`DataF2c`; this crate keeps the established `ControlC2F`/`ControlF2C`/
`DataC2F`/`DataF2C` names. After regenerating, rename those four identifiers in
the generated file (a plain find/replace) before committing.

[`prost`]: https://docs.rs/prost
