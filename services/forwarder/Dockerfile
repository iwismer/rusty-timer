# services/forwarder/Dockerfile
#
# Multi-stage Rust build for the rt-forwarder service.
#
# Stage 1 (builder): Compile the forwarder binary in release mode.
# Stage 2 (runtime): Minimal Debian image with only the binary and config.
#
# Build:
#   docker build -t rt-forwarder -f services/forwarder/Dockerfile .
#
# Run:
#   docker run --rm \
#     -v /etc/rusty-timer:/etc/rusty-timer:ro \
#     -v /var/lib/rusty-timer:/var/lib/rusty-timer \
#     -p 8080:8080 \
#     rt-forwarder

# ---------------------------------------------------------------------------
# Stage 1: Builder
# ---------------------------------------------------------------------------
FROM rust:1.89.0-bookworm AS builder

WORKDIR /build

# Install build dependencies.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests for dependency layer caching.
COPY Cargo.toml Cargo.lock ./
COPY rust-toolchain.toml ./

# Copy all workspace crates (needed for workspace build).
COPY crates/ crates/
COPY services/forwarder/ services/forwarder/
COPY services/server/ services/server/
COPY services/receiver/ services/receiver/
COPY src/ src/

# Build the forwarder binary in release mode.
RUN cargo build --release --package forwarder --bin forwarder

# ---------------------------------------------------------------------------
# Stage 2: Runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root service user.
RUN useradd -r -s /bin/false -m -d /var/lib/rusty-timer rt-forwarder

# Create config and data directories.
RUN mkdir -p /etc/rusty-timer /var/lib/rusty-timer && \
    chown rt-forwarder:rt-forwarder /var/lib/rusty-timer

# Copy the compiled binary.
COPY --from=builder /build/target/release/forwarder /usr/local/bin/rt-forwarder

# Health check: forwarder exposes /healthz and /readyz on status HTTP port.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://localhost:8080/healthz || exit 1

USER rt-forwarder

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/rt-forwarder"]
