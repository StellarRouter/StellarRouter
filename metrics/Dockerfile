# Multi-stage build for minimal final image
FROM rust:1.83-slim as builder

WORKDIR /build

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY contracts ./contracts
COPY metrics ./metrics
COPY api-server ./api-server

# Build the metrics exporter
RUN cargo build --release -p router-metrics-exporter

# Runtime stage
FROM debian:bookworm-slim

# Install CA certificates for HTTPS RPC calls
RUN apt-get update && \
    apt-get install -y ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /build/target/release/router-metrics-exporter /usr/local/bin/

# Create non-root user
RUN useradd -m -u 1000 metrics && \
    chown -R metrics:metrics /usr/local/bin/router-metrics-exporter

USER metrics

EXPOSE 9090

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:9090/health || exit 1

ENTRYPOINT ["router-metrics-exporter"]
