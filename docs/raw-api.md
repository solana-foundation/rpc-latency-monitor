# Raw-data API

Partner-facing read API over the monitor's Prometheus time series. No raw request/response bodies exist anywhere — the prober measures and discards; this exposes the recorded series only.

## Endpoints

`GET /raw/{template}` with `Authorization: Bearer <JWT>`.

| Template | Returns | Extra params |
|---|---|---|
| `latency` | `histogram_quantile` per provider, ms | `q` ⊆ `0.5,0.9,0.95,0.99` (default `0.5,0.95,0.99`) |
| `latency_buckets` | raw histogram bucket increases by provider/le | — |
| `requests` | request counts by provider + status or error_kind | `by=status\|error_kind` (default `status`) |
| `win_rate` | per-provider fastest-at-timestamp share | — |
| `claim_checks` | `rpc_claim_check_total` increases by provider/method/result | no `infra`/`region` (metric has neither) |

Common params: `provider`, `method`, `region` (requires `infra`), `infra` — raw label values as stored (e.g. `region=fra2`, `infra=tsw`); `start`/`end` (unix seconds or RFC 3339, default last 24h); `step` (seconds, min 60, default range/500); `format=json|csv`.

Guards: allowlisted templates only (no free-form PromQL), label values validated against `[A-Za-z0-9_.-]{1,64}`, ≤ 5000 points per query, range ≤ retention (~13 months).

## Auth

HS256 JWT against `RAW_API_JWT_SECRET`; claims `sub` (partner name) and `exp` are mandatory. Mint tokens:

```
RAW_API_JWT_SECRET=$(doppler secrets get RAW_API_JWT_SECRET --project rpc-latency-monitor --config prd --plain) \
  node scripts/mint-raw-api-token.mjs <partner> [days]
```

Revocation = rotate the secret (revokes all tokens).

## Isolation (non-negotiable)

- **Never runs on the fleet boxes or the reference node.** Deploy as its own service on separate compute; a flood of API traffic must not be able to touch probe timing or claim verification. The monitor and this API share a repo and an image, nothing else.
- **Blast radius is Grafana Cloud's query API only.** The service holds a read-only *viewer* token (`RAW_API_GRAFANA_TOKEN`, a dedicated service account — falls back to `GRAFANA_API_TOKEN` only if unset; don't do that in prod) so Grafana-side per-token rate limits sandbox it away from the token the dashboards and alert provisioning use.
- **Flood control:** unauthenticated requests are rejected before any upstream call; 60 req/min per token (fixed window, 429 + Retry-After); at most 8 in-flight upstream queries globally (extra requests get 503 + Retry-After); 30s upstream timeout.

## Running

Env: `RAW_API_JWT_SECRET`, `GRAFANA_API_URL` (`https://rpclatency.grafana.net`), `RAW_API_GRAFANA_TOKEN`, optional `GRAFANA_DATASOURCE_UID` (default `grafanacloud-prom`), `RAW_API_BIND` (default `0.0.0.0:8080`). All secrets in Doppler `rpc-latency-monitor/prd`.

The `raw-api` binary ships in the monitor image (`docker run <image> raw-api` overrides the entrypoint via `--entrypoint raw-api`). Recommended host: Cloud Run (TLS + scale-to-zero, `max-instances=1` keeps the per-token rate limit globally accurate):

```
gcloud run deploy rpc-raw-api --project rpc-latency-monitor --region us-east4 \
  --image us-east4-docker.pkg.dev/rpc-latency-monitor/rpc-latency-monitor/rpc-latency-monitor:latest \
  --command raw-api --max-instances 1 --allow-unauthenticated \
  --set-secrets RAW_API_JWT_SECRET=...,RAW_API_GRAFANA_TOKEN=... --set-env-vars GRAFANA_API_URL=https://rpclatency.grafana.net
```
