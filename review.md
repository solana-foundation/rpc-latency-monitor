# Staff Review — [PRO-1354] Pre-Aggregate Sender Provider Economics Into ClickHouse Rollup

**Verdict: approve-with-nits**

The PR is well-scoped and the intent (move a heavy, public-facing, auto-refreshing `ARRAY JOIN` scan off the raw geyser tables into a per-minute rollup) is sound. The dashboard rewrite faithfully preserves the original column semantics, filters, and `$provider` / `$__timeFilter` / `$__timeInterval` behavior. Aggregate-state combinators are used consistently and correctly (`countState`/`countMerge`, `sumState`/`sumMerge`, `quantileState(0.5)`/`quantileMerge(0.5)`), and the function signatures match the declared `AggregateFunction(...)` column types. The ownership / hand-off framing is explicit and appropriate given this repo never touches the geyser cluster.

No blocking issues. A few accuracy/operational notes the geyser-data team should be aware of before applying.

## Blocking issues

None.

## Non-blocking suggestions

1. **Median is now an approximation across buckets — call it out at the dashboard, not just in docs.**
   `clickhouse/sender_provider_rollup.sql:43-46` (MV) stores `quantileState(0.5)` per 1-minute bucket; the dashboard merges those states across the window (`grafana/sender-dashboard.json:43`). Merging per-minute `quantile` reservoir states is *not* equal to `median()` over all raw rows — `quantile` uses reservoir sampling, so the merged result is approximate and slightly non-deterministic, whereas the old `median(...)` over the full window was effectively exact for typical volumes. The SQL header and README do note "(approximate) median", which is good. Suggestion: rename the dashboard column from `"Median tip (lamports)"` to `"Median tip (approx, lamports)"` (or similar) so the public dashboard doesn't imply exactness it no longer has. If exactness matters here, `quantileExactState`/`quantileExactMerge` would be exact at higher memory cost.

2. **Pre-existing fanout double-counting of `fee_sol` is carried forward (not introduced here).**
   In both the old and new queries, `fee_sol` is summed once per `(tx, matching tip account)` row produced by the `ARRAY JOIN ... INNER JOIN tip_accounts` (`sender_provider_rollup.sql:42-50`). A transaction that writes to N tip accounts each with a positive delta contributes its fee N times. The rollup correctly *preserves* the existing behavior, so this is not a regression — but since this PR is the one formalizing these economics into a durable rollup, it's a good moment to confirm with the geyser-data / Sender owners that per-tip-account fanout of `total_fees_sol` (and the tx `count`) is the intended definition. If "fees per provider" is meant to be per-transaction, the rollup will bake in the inflated value for 30 days of history. Worth one line of confirmation before the team applies it.

3. **Bucket time source vs. event time.**
   `bucket` is derived from `gt.ingested_at` (`sender_provider_rollup.sql:39`), matching the original query's `$__timeFilter(gt.ingested_at)`, so this is behavior-preserving. Just flag for the owners: the rollup keys economics on *ingest* time, not block/slot time, so late-arriving or backfilled rows land in their ingest minute. Fine for a "recent activity" dashboard; document the choice so nobody later assumes block-time semantics.

4. **Backfill + MV creation ordering can drop or double-count rows at the boundary.**
   The README hand-off (`clickhouse/README.md:30-34`) lists "create MV" then "optional backfill `>= now() - 30 DAY`". If the MV is created first and then the backfill runs with `ingested_at >= now() - 30 DAY`, rows ingested in the window between MV creation and backfill execution are counted twice. Recommend the checklist make the ordering/boundary explicit: either create the MV at time T and backfill strictly `ingested_at < T`, or backfill first then create the MV. A one-line note prevents a real data-quality bug at deploy time.

## Nits

1. **Column name plural mismatch.** `tip_quantiles_lamports` / `fee_quantiles_sol` (`sender_provider_rollup.sql:30-31`) are plural "quantiles" but each holds a single `quantile(0.5)` state. `median_tip_state_lamports` / `median_fee_state_sol` would read truer. Minor.

2. **DRY the backfill SELECT.** The optional backfill (`sender_provider_rollup.sql:74-94`) duplicates the MV's SELECT verbatim except for the `ingested_at` bound. That's acceptable for a one-shot operational snippet, but a comment noting "keep in sync with the MV SELECT above" would help whoever edits one and forgets the other.

3. **README query window uses a literal, dashboard uses `$__timeFilter`.** The README example (`clickhouse/README.md:55`) filters `bucket >= now() - INTERVAL 30 MINUTE`; harmless, but adding a sentence that the live dashboard uses Grafana's `$__timeFilter(bucket)` (not a fixed 30m) avoids confusion about where the "30 minute" default actually comes from.

4. **No Rust touched — repo Rust style guide N/A.** Confirmed: no `.unwrap()`/`.expect()` / server-code concerns apply; changes are SQL + dashboard JSON + docs only.
