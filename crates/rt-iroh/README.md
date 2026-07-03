# rt-iroh

Shared iroh endpoint wrapper for the P2P data/control plane.

The crate centralizes endpoint construction, deterministic loopback options,
ALPN wiring, and direct-address injection used by the forwarder, receiver, and
P2P test utilities.
