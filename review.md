# Staff Review — PR #13: Rewrite README in OSS Observability Style

**Verdict: approve-with-nits**

Docs-only change (README.md, +233/-36). I fact-checked every concrete claim
against the source tree (`src/`, `config.example.yaml`, `grafana/`, `deploy/`,
`.github/workflows/`). The rewrite is unusually well-grounded — nearly every
statement maps to real code. No blocking issues. A few small accuracy nits below.

## Blocking issues

None.

## Non-blocking suggestions

1. **Checks table — metric `method` label vs config value mismatch is a latent
   foot-gun.** `README.md` "Checks" table (the `### Checks` section) lists the
   config `method:` values (`get_block_recent`, `get_transaction_recent`, etc.)
   mapped to JSON-RPC calls. That column is correct *as config values*. But the
   Prometheus `method` **label** emitted for those two is `getBlock_recent` and
   `getTransaction_recent` (see `RpcMethod::label()` in
   `src/rpc/methods.rs:30-41`) — which equals neither the config name nor the
   `getBlock`/`getTransaction` rpc name shown in the table. Someone reading the
   table and then querying Grafana with `method="get_block_recent"` (or
   `"getBlock"`) will get nothing. Consider a one-line note in the metrics section
   that the `method` label uses the `label()` form (camelCase, with `_recent`
   suffix for the recent-block/tx checks). Strictly the table is in-scope correct
   since it says "the `method:` value," so this is a clarity nit, not an error.

2. **`rpc_up` label set.** Metrics table (`README.md`) describes `rpc_up` fine
   ("most recent check succeeded"), but it may help to note it is labeled by
   `provider` only (no `method`) — see `src/metrics.rs:48-50` and `record_call`.
   Because every method's result writes the same `provider` series, the value is
   effectively last-writer-wins across methods. The current wording is accurate;
   adding the label scope would preempt confusion when reading the panel queries.

## Nits

- **CI badge / job name consistency.** Badge points to `ci.yml` (job name `CI`) —
  correct. The Deployment diagram's `verify` step text matches the *deploy*
  workflow's verify job. Both sets of commands match exactly
  (`cargo fmt --all --check`, `clippy ... -D warnings`, `cargo test --all-features`).
  No change needed; noting that I verified it.

## Verified-accurate (spot list)

- Metrics names/types/labels (`rpc_latency_seconds` histogram; `rpc_slot_lag`,
  `rpc_up` gauges; `rpc_requests_total` counter; const `region` label) — match
  `src/metrics.rs`.
- error_kind values `timeout/transport/http_status/rpc_error/decode` — match
  `ErrorKind::as_str()` in `src/rpc/mod.rs`.
- `Cache-Control: no-cache` + `Pragma: no-cache` — match `RpcClient::new`
  (`src/rpc/mod.rs`).
- GPA: token program, `dataSlice {offset:0,length:0}`, `memcmp` owner filter,
  `withContext` — match `src/rpc/methods.rs`.
- `get_block_recent` = tip − 32 (`BLOCK_CONFIRMATION_DEPTH`); `get_transaction_recent`
  fed by `get_signatures_for_address` — match `src/rpc/methods.rs`.
- `reference_slot.source: max_observed | endpoint`, `request_timeout: 10s`,
  `server.bind 0.0.0.0:9464`, humantime durations — match `config.example.yaml`.
- `/metrics` and `/health` routes — match `src/server.rs:13-14`.
- Regions list — exactly matches `locations` map in
  `deploy/gcp/terraform/variables.tf:7-15`.
- `e2-small`, Container-Optimized OS — match `variables.tf:20` and
  `instances.tf` cos-stable image.
- Deploy pipeline: WIF auth, Cloud Build (`gcloud builds submit --config
  cloudbuild.yaml`), terraform apply, staggered reset with `RESET_DELAY` +
  `sleep`, Slack started/succeeded/failed — match `.github/workflows/deploy.yml`
  and `deploy/gcp/deploy.sh:40-62`.
- workflow_dispatch targets `all/gcp/grafana` — match `deploy.yml:11`.
- Dashboard panels (p99/p50/Slot lag/Win %) and `Sender` board — match
  `grafana/dashboard.json` and `grafana/sender-dashboard.json` titles.
- Alloy 15s scrape of `127.0.0.1:9464`, `GRAFANA_CLOUD_*` env — match
  `grafana/alloy-config.alloy`.
- `.env.example` keys (HELIUS_API_KEY, TRITON_*, QUICKNODE_URL, GRAFANA_CLOUD_*),
  `rust-toolchain.toml` stable, Apache-2.0 LICENSE — all present and correct.

Nice work — this reads like a real OSS project README and the claims hold up.
