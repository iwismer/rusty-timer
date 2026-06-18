# rt-test-utils

Shared test helpers for the Rusty Timer P2P stack.

## Contents

- `p2p::MockForwarder` helpers for deterministic receiver-side P2P tests.
- Loopback iroh helpers with relay/discovery disabled and injected addresses.
- `poll_until` for bounded async assertions.

The helpers are intentionally local/deterministic so unit and seam tests can run
without external infrastructure.
