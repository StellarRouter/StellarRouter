# syntax=docker/dockerfile:1
FROM rust:1.78-slim AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY api-server/ api-server/

RUN cargo build --release -p router-api-server

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/router-api-server /usr/local/bin/

# Create non-root user
RUN useradd -m -u 1000 apiserver && \
    chown -R apiserver:apiserver /usr/local/bin/router-api-server

USER apiserver

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

ENTRYPOINT ["router-api-server"]
