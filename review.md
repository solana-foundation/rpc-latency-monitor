# Staff Review — [PRO-1346] Add Grafana Alert Rules as Code (PR #16)

**Verdict: approve-with-nits**

Solid, well-scoped "alerting as code" increment. The three rules map cleanly to metrics the
monitor actually exports (`src/metrics.rs`), the placeholder substitution reuses existing env with
no new secrets, and the PUT-then-POST upsert is the correct Grafana provisioning pattern. Queries
are label-correct: `region` is a registry-wide const label (`src/metrics.rs:27-28`) so
`by (provider, region)` grouping resolves on every series, including `rpc_up` which only carries
`provider` as a per-series label. No Rust touched. The items below are non-blocking.

## Blocking issues

None.

## Non-blocking suggestions

1. **`noDataState: NoData` contradicts the README's coverage claim** — `grafana/alerts/provider-down.json:9`
   and `grafana/alerts/success-rate-dropping.json:9`. The README states the provider-down rule
   covers the "provider stops being scraped / no traffic" case for success-rate
   (`grafana/alerts/README.md`, success-rate bullet). But provider-down sets `noDataState: NoData`,
   so when `rpc_up` series disappear entirely (scrape gap, VM down, region offline) the rule goes to
   NoData rather than Alerting — the exact total-outage case nobody gets paged for. If the intent is
   "absent metrics = page someone," at least one rule (likely provider-down) should use
   `noDataState: Alerting`. Worth a deliberate decision and a one-line comment in the README.

2. **PUT failure is fully silenced, masking real errors** — `deploy/gcp/deploy.sh:53-58`. The PUT uses
   `--fail-with-body` but redirects `2>/dev/null` and `>/dev/null`, so a 400 (malformed payload),
   401/403 (bad token/permissions), or 5xx all look identical to "rule doesn't exist yet" and silently
   fall through to POST. On a genuine update path a transient 5xx would trigger a spurious create
   attempt. Consider distinguishing 404 (→ POST create) from other failures (→ surface the body and
   fail), e.g. capture the HTTP status with `-w '%{http_code}'` and only POST on 404. At minimum, drop
   the `2>/dev/null` so the PUT error body is visible when the POST fallback also fails.

3. **`push_alerts` is only reachable via `push_dashboards`** — `deploy/gcp/deploy.sh:34` calls
   `push_alerts` at the end of `push_dashboards`, and the `case` only dispatches `push_dashboards`
   (line ~68). Functionally correct, but coupling alert pushes inside the dashboard function is
   surprising and not self-documenting. Cleaner to call both explicitly from the `grafana)`/`all)`
   case arms (`push_dashboards; push_alerts`) so the dispatch table reflects what runs.

## Nits

1. **`slot-lag` double-windowing is intentional but undocumented** — `grafana/alerts/slot-lag-elevated.json`:
   `relativeTimeRange.from: 600` (10m) wraps an `avg_over_time(rpc_slot_lag[5m])` instant query whose
   result is then `reduce: last`. The 10m relative range is effectively inert for an `instant: true`
   query (only the evaluation instant matters; the `[5m]` range vector does the smoothing). Harmless,
   but the mismatched 600s vs `[5m]` reads like a bug to the next person — a comment or aligning the
   range would help. Same inert-`relativeTimeRange` note applies to `provider-down.json` (`from: 300`).

2. **`maxDataPoints: 43200` / `intervalMs: 1000` are dashboard-panel defaults** carried into alert-rule
   models across all three files. They don't affect an `instant` alert query but are noise; safe to drop.

3. **`rpc_slot_lag` carries a `method` label** (`src/metrics.rs:40`) that the query aggregates away with
   `max by (provider, region)`. Correct, but worth a one-word note in the README that the max is across
   methods, since only slot-bearing methods populate the gauge.

---
Reviewed against repo Rust style guide (no Rust changed) and the Code Review Checklist
(failure modes, error handling, operational visibility). Scope and intent match PRO-1346.
