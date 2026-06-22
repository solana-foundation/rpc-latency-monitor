# Staff Review — PR #12 [PRO-1345] Add Per-Region Provider Win-Rate Panel and Alert

**Verdict: changes-requested**

The dashboard panel is solid and ships as intended. The alert rule, however, is never wired into Grafana provisioning, so it will silently do nothing once merged — the headline half of this PR is non-functional.

---

## Blocking issues

### 1. The alert rule is never provisioned — it does nothing
`grafana/alerts/win-rate.json` (new file) + `deploy/docker-compose.yaml:32-34`

The Grafana container only mounts two paths:
- `./grafana/provisioning` → `/etc/grafana/provisioning` (dashboards + datasources providers)
- `../grafana/dashboard.json` → `/var/lib/grafana/dashboards/dashboard.json`

The new `grafana/alerts/win-rate.json` is mounted nowhere. Grafana provisions alert rules only from files under `/etc/grafana/provisioning/alerting/`. As written, this file is dead weight: the alert is never loaded, never evaluated, never fires. The PR title and summary lead with "and Alert," but the alert is inert.

Fix: add a volume mount that lands this file under the provisioning alerting dir, e.g.
```yaml
- ../grafana/alerts:/etc/grafana/provisioning/alerting:ro
```
and confirm the file conforms to the alerting provisioning schema (it currently uses the `groups`/`rules` format, which is correct for `/provisioning/alerting/`). Please verify end-to-end that the rule actually shows up under Alerting → Alert rules after `docker compose up`, not just that the JSON parses. The PR's "Validation" section only claims the JSON parses — that does not validate provisioning.

### 2. Datasource `uid` pin will break the existing dashboard's datasource picker
`deploy/grafana/provisioning/datasources/prometheus.yml:6`

Pinning `uid: prometheus` is the right call to let the alert reference the datasource deterministically. But the alert's data node A also hardcodes `"datasourceUid": "prometheus"` and `{ "uid": "prometheus" }`, while the dashboard uses the templated `${datasource}` variable. After this change there are effectively two contracts (templated picker vs. hardcoded uid). That's tolerable, but please confirm the existing dashboard still resolves `${datasource}` to the now-uid-pinned datasource on a fresh provision (clean volume), since changing a provisioned datasource's uid on an existing deployment can leave dashboards pointing at a stale/missing uid. Recommend testing against a fresh Grafana volume and noting it in the PR.

---

## Non-blocking suggestions

### 3. Alert metric is hardcoded to `getLatestBlockhash`; dashboard panel is templated
`grafana/alerts/win-rate.json` (expr + `summary`)

The panel works for any `$method`, but the alert only ever watches `getLatestBlockhash`. That's a reasonable scoping choice, but it means a regression on any other method goes unalerted. If `getLatestBlockhash` is intentionally the canary, add a one-line comment/annotation saying so; otherwise consider templating per method or documenting the gap in the ticket.

### 4. `noDataState: NoData` on a per-region max can be noisy for low/zero-traffic regions
`grafana/alerts/win-rate.json` (rule `noDataState`)

If a region temporarily has no samples (deploy, scrape gap), the rule flips to NoData and, depending on contact-point config, can page. Given the existing CLAUDE.md guidance that "gaps in data may be buffering artifacts, not actual outages," consider `noDataState: OK` or `KeepLast` unless a missing region is itself actionable.

### 5. The win-rate expression is duplicated verbatim between panel 7, new panel 8, and the alert
`grafana/dashboard.json:112`, `grafana/dashboard.json` new panel, `grafana/alerts/win-rate.json` expr

The `rate(_sum)/rate(_count) == bool on (region) group_left() min by (region) (...)` block is now copy-pasted three times with subtle differences ($__rate_interval vs 5m, avg-by-provider vs avg-by-provider-region vs max-by-region). This is fragile: a fix to the win logic must be applied in three places. Not blocking for a Grafana JSON repo (no templating mechanism), but worth a comment in each target noting they must stay in sync, or a generator script if this keeps growing.

---

## Nits

### 6. Alert query uses fixed `[5m]` rate window while dashboard uses `$__rate_interval`
`grafana/alerts/win-rate.json` (expr A)

Minor inconsistency. The alert can't use `$__rate_interval` (no dashboard context), so `[5m]` is a fine concrete choice — just flagging that alert and panel may diverge slightly under different scrape intervals.

### 7. Annotation interpolation depends on reduce node label propagation
`grafana/alerts/win-rate.json` (`description` uses `$values.B.Value` and `$labels.region`)

`$labels.region` relies on the `region` label surviving through the reduce node B, which it does for `max by (region)`. Worth a quick visual confirmation in the Grafana UI that both `{{ $labels.region }}` and `{{ printf "%.1f" $values.B.Value }}` actually render in a test fire, since silent template-eval failures just drop the value.

---

## What's good
- Panel 8 layout (`gridPos y=16, h=10, w=24`) does not overlap existing panels (row ends at y=16); id 8 is unique.
- The per-region win logic correctly keeps `region` in the output grouping (`avg by (provider, region)`, legend `{{region}} / {{provider}}`) — matches the stated intent and mirrors the proven panel 7 approach.
- Reuses existing `${datasource}` / `$method` / `$region` / `$provider` template variables rather than inventing new ones.
- No Rust touched; nothing to flag against rust-services style rules.
