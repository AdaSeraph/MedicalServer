# Multi-stage production Dockerfile for medical-server
FROM rust:1-slim-bookworm AS builder

WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
COPY config ./config

RUN cargo build --release --bin medical-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -m medical

WORKDIR /app
COPY --from=builder /app/target/release/medical-server /usr/local/bin/medical-server
COPY --from=builder /app/migrations /app/migrations
COPY --from=builder /app/config /app/config

USER medical
ENV SERVER_ADDR=0.0.0.0:8080
ENV RUST_LOG=info

EXPOSE 8080

HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/health || exit 1

ENTRYPOINT ["/usr/local/bin/medical-server"]
