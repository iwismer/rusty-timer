# Network Architecture

## Production Layout

```
                    ┌─ Field Site A (LAN) ─┐
IPICO Reader ─TCP─► Forwarder (Pi)         │
  :10000            :80 (status UI)        │
                    │                      │
                    └──── WAN/Internet ────┘
                              │
                              ▼ WSS :443
                    ┌── Cloud / Server ───┐
                    │  Caddy (:80/:443)   │
                    │    │                │
                    │    ▼                │
                    │  rt-server (:8080)  │
                    │    │                │
                    │    ▼                │
                    │  PostgreSQL (:5432) │
                    └─────────────────────┘
                              │
                              ▼ WSS :443
                    ┌── Timing Tent ──────┐
                    │  Receiver           │
                    │  :9090 (control UI) │
                    │  :10000+ (TCP out)  │
                    │    │                │
                    │    ▼                │
                    │  Timing Software    │
                    │  (IPICO Connect)    │
                    └─────────────────────┘
```

## Ports

| Component | Port | Protocol | Direction | Notes |
|-----------|------|----------|-----------|-------|
| IPICO Reader | 10000 | TCP | Reader → Forwarder | Standard IPICO reader port |
| Forwarder status | 80 (default on SBC) or 8080 | HTTP | LAN only | Health check + embedded UI |
| Server | 8080 | HTTP/WS | Inbound from forwarders + receivers | Put behind a reverse proxy for TLS |
| Reverse proxy | 80, 443 | HTTPS/WSS | Public | Caddy, nginx, etc. |
| PostgreSQL | 5432 | TCP | Server → Postgres | Internal only; never expose publicly |
| Receiver control | 9090 | HTTP | Localhost only | Receiver UI + control API |
| Receiver TCP out | 10000+ | TCP | Localhost only | One port per subscribed stream |

## Firewall Rules

### Server host

- Allow inbound **443** (HTTPS/WSS) from forwarders and receivers
- Allow inbound **80** (HTTP redirect to HTTPS, optional)
- Block inbound **5432** (Postgres) and **8080** (direct server) from public networks

### Forwarder (field site)

- Allow outbound **443** to server
- Allow inbound TCP from IPICO reader (usually same LAN)
- Allow inbound **80** (status UI) from trusted LAN only

### Receiver (timing tent)

- Allow outbound **443** to server
- No inbound ports needed from external networks (receiver binds to localhost only)

## TLS / Reverse Proxy

The server does not terminate TLS itself. Use a reverse proxy (Caddy, nginx, etc.) in front of the server for HTTPS/WSS.

See the [Caddy + Authelia example](../deploy/server/README.md#caddy--authelia-example) for a production reverse proxy configuration with authentication.
