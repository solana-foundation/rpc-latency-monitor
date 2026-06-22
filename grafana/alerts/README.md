# Grafana alerting as code

A single alert rule for the RPC latency monitor, stored as JSON in Grafana's
`apiVersion 1` alert-rule groups provisioning format (`monitor-data.json`). It is
upserted into the Grafana Cloud stack via the provisioning API by the deployer —
see "Deployment" below.

The monitor only needs one thing to be true: the dashboards are up and showing
data. We intentionally do **not** alert on any individual provider being down,
slow, or losing — only on the metrics pipeline going dark.

## Rule

| File | Severity | Fires when |
| --- | --- | --- |
| `monitor-data.json` | critical | `count(rpc_up) < 1` (no series at all) for 10m |

- **rpc-latency monitor not reporting (dashboard has no data)** — query `A` is
  `count(rpc_up)` against the Prometheus datasource, reduced to its last value
  (`refId: B`), and the threshold (`refId: C`) fires when that count `is below 1`.
  When the monitor VM, the scrape, or remote-write stops, the `rpc_up` series
  disappear entirely and the dashboards go blank. `noDataState: Alerting` plus a
  `for: 10m` window means total metric loss pages rather than going silently to
  NoData, while riding out brief gaps. There are no per-provider labels.

## Placeholders

The JSON contains placeholders that the deployer substitutes at push time using the
existing Grafana env/secret pattern — no new secrets are introduced:

- `${GRAFANA_DATASOURCE_UID}` — UID of the Prometheus datasource in the Grafana stack
  (config, not a secret; defaults to `prometheus` if unset).
- `${GRAFANA_FOLDER_UID}` — folder the rule group lives in (reuses the existing
  `GRAFANA_FOLDER_UID` used for dashboards; empty means the General folder).

## Deployment

`deploy/gcp/deploy.sh` (run via `TARGET=grafana` or `all`) pushes both the dashboards
(`push_dashboards`) and this alert rule (`push_alerts`) as separate steps. It
substitutes the placeholders, reshapes the `apiVersion 1` group into the rule-group
provisioning body, and upserts it via
`PUT /api/v1/provisioning/folder/{folderUid}/rule-groups/{group}`, reusing
`GRAFANA_API_URL` / `GRAFANA_API_TOKEN`. The PUT replaces the group's rules so
re-running the deploy can't create duplicates. The `X-Disable-Provenance` header
keeps the rule editable as provisioned-via-API rather than file-locked.
