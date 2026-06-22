# Grafana alerting as code

Alert rules for the RPC latency monitor, stored as JSON in Grafana's alert-rule
provisioning format (one file per rule). They are upserted into the Grafana Cloud
stack via the provisioning API by the deployer — see "Deployment" below.

Each rule evaluates a Prometheus query (`refId: A`) against the metrics the monitor
exports, reduces it to the latest value (`refId: B`), and fires when the threshold
condition (`refId: C`) holds for the rule's `for` duration. Rules are grouped under
`ruleGroup: rpc-latency-monitor` and labeled `service: rpc-latency-monitor`.

## Rules

| File | Severity | Fires when |
| --- | --- | --- |
| `provider-down.json` | critical | `min by (provider, region) (rpc_up) < 1` for 5m |
| `slot-lag-elevated.json` | warning | `avg_over_time(rpc_slot_lag[5m]) > 150` slots per provider/region for 10m |
| `success-rate-dropping.json` | warning | success ratio from `rpc_requests_total` `< 0.95` per provider/region for 10m |

- **Provider down** — `rpc_up` is `1` on a successful check and `0` on failure. The
  rule takes the `min` per provider/region so any failing region trips it. A 5m `for`
  window rides out single transient check failures.
- **Slot lag elevated** — `rpc_slot_lag` is how far a provider's reported slot trails
  the max-observed chain tip. 150 slots is roughly 60s behind. Smoothed with a 5m
  average so brief congestion spikes don't alert.
- **Success rate dropping** — derived from the `rpc_requests_total` counter as
  `rate(status="success") / rate(total)` per provider/region over 10m. `clamp_min`
  on the denominator avoids divide-by-zero when a provider stops being scraped (that
  case is covered by the provider-down rule instead).

## Placeholders

The JSON contains placeholders that the deployer substitutes at push time using the
existing Grafana env/secret pattern — no new secrets are introduced:

- `${GRAFANA_DATASOURCE_UID}` — UID of the Prometheus datasource in the Grafana stack
  (config, not a secret; defaults to `prometheus` if unset).
- `${GRAFANA_FOLDER_UID}` — folder the rules live in (reuses the existing
  `GRAFANA_FOLDER_UID` used for dashboards; empty means the General folder).

## Deployment

`deploy/gcp/deploy.sh push_dashboards` (run via `TARGET=grafana` or `all`) pushes both
the dashboards and these alert rules. For each rule it substitutes the placeholders and
upserts via `PUT /api/v1/provisioning/alert-rules/{uid}` (falling back to `POST` to
create), reusing `GRAFANA_API_URL` / `GRAFANA_API_TOKEN`. The `X-Disable-Provenance`
header keeps the rules editable as provisioned-via-API rather than file-locked.
