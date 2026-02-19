# services/server/Dockerfile
#
# Multi-stage build for the rt-server service.
#
# Stages:
#   1. dashboard-builder (node): Builds the SvelteKit dashboard static files.
#   2. rust-builder: Compiles the server binary in release mode.
#   3. runtime: Minimal Debian image with server binary + dashboard static files.
#
# The server binary serves the dashboard from DASHBOARD_DIR (/srv/dashboard).
#
# Build:
#   docker build -t rt-server -f services/server/Dockerfile .
#
# Run (see deploy/docker-compose.prod.yml for full stack):
#   docker run --rm \
#     -e DATABASE_URL="postgres://user:pass@db:5432/rtserver" \
#     -e BIND_ADDR="0.0.0.0:8080" \
#     -p 8080:8080 \
#     rt-server

# ---------------------------------------------------------------------------
# Stage 1: Dashboard builder (SvelteKit + adapter-static)
# ---------------------------------------------------------------------------
FROM node:20-bookworm-slim AS dashboard-builder

WORKDIR /dashboard

# Copy the dashboard application source.
COPY apps/server-ui/ ./

# Install dependencies and build static output.
RUN npm ci && npm run build

# ---------------------------------------------------------------------------
# Stage 2: Rust builder
# ---------------------------------------------------------------------------
FROM rust:1.89.0-bookworm AS rust-builder

WORKDIR /build

# Install build dependencies.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests.
COPY Cargo.toml Cargo.lock ./
COPY rust-toolchain.toml ./
COPY services/server/.sqlx/ services/server/.sqlx/

# Copy all workspace crates.
COPY crates/ crates/
COPY services/forwarder/ services/forwarder/
COPY services/server/ services/server/
COPY services/receiver/ services/receiver/
COPY src/ src/

# Build the server binary in release mode.
RUN cargo build --release --package server --bin server

# ---------------------------------------------------------------------------
# Stage 3: Runtime image
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root service user.
RUN useradd -r -s /bin/false rt-server

# Dashboard static files go into /srv/dashboard.
# The server binary serves them via the configured static file path.
RUN mkdir -p /srv/dashboard
COPY --from=dashboard-builder /dashboard/build /srv/dashboard
RUN chown -R rt-server:rt-server /srv/dashboard

# Copy the server binary.
COPY --from=rust-builder /build/target/release/server /usr/local/bin/rt-server

# Health check.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -fsS http://localhost:8080/healthz || exit 1

USER rt-server

EXPOSE 8080

# Required env vars: DATABASE_URL, BIND_ADDR (default 0.0.0.0:8080)
ENTRYPOINT ["/usr/local/bin/rt-server"]
