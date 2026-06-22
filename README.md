# rpc-latency-monitor

A neutral, read-only latency monitor for Solana RPC providers.

Built and operated by the Solana Foundation as part of the **VIP Trading Program** to act as an
independent measurement party: we benchmark provider read performance and publish the data, rather
than running RPC infrastructure ourselves. This is **v0** — read-only RPC latency + slot lag.

## What it does

For every configured provider, the monitor runs a set of read RPC checks on independent loops, times
each request, and records:

- **Request latency** (`rpc_latency_seconds`) — wall-clock round trip per provider × method.
- **Slot lag** (`rpc_slot_lag`) — how far each provider's reported slot trails the observed chain tip.
- **Request outcomes** (`rpc_requests_total`, `rpc_up`) — success/error counts and liveness.

Metrics are exposed in Prometheus format. A Grafana Alloy agent scrapes them and `remote_write`s to the
Foundation's public Grafana Cloud stack, where the dashboard renders p50/p90/p99 latency, slot lag, and
success rate per provider.

> Not in v0: transaction sending / landing-service latency, recommendation API, multi-provider SDK.

## Quickstart (local)

```bash
cp config.example.yaml config.yaml   # edit providers/checks

# Secrets via Doppler (source of truth):
doppler login                        # one-time browser OAuth
deploy/run-with-doppler.sh cargo run --release -- --config config.yaml

# ...or, without Doppler, a local .env:
cp .env.example .env                 # fill in provider credentials
set -a; source .env; set +a
cargo run --release -- --config config.yaml

curl localhost:9464/metrics
```

Or bring up the full local stack (monitor + Prometheus + Grafana, dashboard auto-provisioned at
http://localhost:3000):

```bash
cp config.example.yaml config.yaml   # required: mounted into the monitor container
docker compose -f deploy/docker-compose.yaml up --build
```

## Configuration & secrets

See [`config.example.yaml`](./config.example.yaml). Provider URLs use `${ENV_VAR}` placeholders so no
secrets live in the config file. The reference "tip" slot defaults to the max `processed` slot observed
across all providers, so slot-lag does not depend on any single provider.

## Deployment

Multi-region on GCP: one small VM per region (Terraform, `deploy/gcp/`), each tagged with its region and
running the monitor plus an Alloy agent that ships metrics to Grafana Cloud. See `deploy/gcp/README` once
that increment lands.

Grafana Cloud stack: https://rpclatency.grafana.net. Import `grafana/dashboard.json` there, and grab the
Prometheus `remote_write` URL + token from
https://rpclatency.grafana.net/connections/add-new-connection/ into the `GRAFANA_CLOUD_*` env vars that
`grafana/alloy-config.alloy` reads.

Publishing the dashboard publicly (read-only share or snapshot, fronted by Cloudflare, with `/metrics`
kept localhost-only): see [`docs/public-hardening.md`](./docs/public-hardening.md) and the committable
Cloudflare WAF / rate-limit / cache rules in [`deploy/cloudflare/waf-rules.json`](./deploy/cloudflare/waf-rules.json).
None of this flips anything public on its own.

## License

Apache-2.0. See [LICENSE](./LICENSE).
