FROM rust:1.89-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ src/

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/pubky-web-index /usr/local/bin/pubky-web-index

RUN mkdir -p /data
ENV DB_PATH=/data/webindex.db

EXPOSE 8080

CMD ["pubky-web-index", "-c", "/dev/null", "daemon"]
