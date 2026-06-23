# services/server/Dockerfile
#
# Multi-stage build for the Rusty Timer coordination server.
#
# The server uses SQLite for durable registry/status state and embeds the
# SvelteKit server UI into the Rust binary via the `embed-ui` feature.
#
# Build from the repository root:
#   docker build -t rt-server -f services/server/Dockerfile .

# ---------------------------------------------------------------------------
# Stage 1: Build the SvelteKit server UI.
# ---------------------------------------------------------------------------
FROM node:24-bookworm-slim AS ui-builder

WORKDIR /build

COPY package.json package-lock.json ./
COPY .npmrc ./
COPY apps/shared-ui/package.json apps/shared-ui/package.json
COPY apps/server-ui/package.json apps/server-ui/package.json

RUN npm ci --workspace apps/shared-ui --workspace apps/server-ui

COPY apps/shared-ui/ apps/shared-ui/
COPY apps/server-ui/ apps/server-ui/

RUN npm run build --workspace apps/server-ui

# ---------------------------------------------------------------------------
# Stage 2: Compile the Rust server with the UI embedded.
# ---------------------------------------------------------------------------
FROM rust:1.93.1-bookworm AS rust-builder

WORKDIR /build

# SQLite is compiled from source via rusqlite's bundled feature. TLS uses
# rustls, so no OpenSSL development package is required.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
COPY services/ services/
COPY apps/receiver-ui/src-tauri/ apps/receiver-ui/src-tauri/
COPY src/ src/
COPY --from=ui-builder /build/apps/server-ui/build apps/server-ui/build

RUN cargo build --release --package server --bin server --features embed-ui

# ---------------------------------------------------------------------------
# Stage 3: Minimal runtime image.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -r -s /bin/false -m -d /var/lib/rusty-timer-server rt-server
RUN install -d -o rt-server -g rt-server -m 0750 /var/lib/rusty-timer-server

COPY --from=rust-builder /build/target/release/server /usr/local/bin/server

ENV BIND_ADDR=0.0.0.0:8080
ENV SERVER_DB_PATH=/var/lib/rusty-timer-server/server.sqlite3
ENV LOG_LEVEL=info

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://localhost:8080/healthz || exit 1

USER rt-server

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/server"]
