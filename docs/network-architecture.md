# Network Architecture

## Production Layout

```
                    ┌─ Field Site A (LAN) ─┐
IPICO Reader ─TCP─► Forwarder (Pi)         │
  :10000            :80/8080 status UI     │
                    │                      │
                    └──── WAN/Internet ────┘
                              │
                              ▼ iroh QUIC/P2P
                    ┌── Timing Tent ──────┐
                    │  Receiver           │
                    │  (Tauri/headless)   │
                    │  :10000+ TCP out    │
                    │    │                │
                    │    ▼                │
                    │  Timing Software    │
                    └─────────────────────┘

                    ┌── Coordination ─────┐
                    │  Thin node          │
                    │  registry/status    │
                    │  allow-list distro  │
                    └─────────────────────┘
```

The thin node coordinates endpoint registration, allow-list distribution, and
status/announcer state. Chip-read events flow directly between forwarders and
receivers over iroh; they are not relayed through the thin node.

## Ports

| Component | Port | Protocol | Direction | Notes |
|-----------|------|----------|-----------|-------|
| IPICO Reader | 10000 | TCP | Reader → Forwarder | Standard IPICO reader port |
| Forwarder status | 8080 default | HTTP | Trusted LAN or localhost | Health check + embedded UI |
| Forwarder P2P endpoint | OS-assigned/QUIC | iroh | Receiver ↔ Forwarder | Uses endpoint IDs and allow-list enforcement |
| Thin node | 8080 default | HTTPS/HTTP behind proxy | Forwarders, receivers, operators | Registry, allow-list, status board |
| Receiver TCP out | 10000+ | TCP | Localhost only | One port per subscribed stream |
| Receiver test bridge | loopback only | HTTP | Local test harness | Compiled only with `test-bridge` |

## Firewall Rules

### Thin-node host

- Allow inbound operator/admin traffic only through the configured reverse proxy.
- Allow M2M bearer-token traffic from provisioned forwarders and receivers.
- Keep public read-only status paths separate from admin/provisioning paths.
- Never expose endpoint identity material over unauthenticated plain HTTP.

### Forwarder field site

- Allow outbound traffic needed for thin-node registration/allow-list polling and
iroh connectivity.
- Allow inbound TCP from the local IPICO reader.
- Restrict status UI access to the trusted local network.

### Receiver timing tent

- Allow outbound traffic needed for thin-node lookup and iroh connectivity.
- Bind replay TCP ports to localhost for timing software on the same machine.
- Keep the `test-bridge` disabled in release builds.

## Deterministic Test Mode

CI and local deterministic E2E tests run iroh loopback-only with relay and
discovery disabled, seeded endpoint keys, and injected local addresses. NAT and
relay validation are lab/user-run lanes and do not gate normal PR checks.
