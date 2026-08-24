FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --shell /usr/sbin/nologin monitor
USER monitor
COPY --from=builder /app/target/release/rpc-latency-monitor /usr/local/bin/rpc-latency-monitor
COPY --from=builder /app/target/release/raw-api /usr/local/bin/raw-api
ENTRYPOINT ["rpc-latency-monitor"]
