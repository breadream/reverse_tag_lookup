# syntax=docker/dockerfile:1

FROM rust:1.88-slim AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/reverse_tag_lookup /usr/local/bin/reverse_tag_lookup
COPY frontend ./frontend

ENV HOST=0.0.0.0
ENV PORT=3000
ENV CACHE_PATH=/app/data/problem-cache.json
ENV RUST_LOG=reverse_tag_lookup=info,tower_http=info

EXPOSE 3000
VOLUME ["/app/data"]

CMD ["reverse_tag_lookup"]
