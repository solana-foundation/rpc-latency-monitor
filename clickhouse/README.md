# ClickHouse rollups

SQL definitions for the small, pre-aggregated tables that the public Grafana
dashboards read from, instead of scanning the raw production geyser tables.

## Why

The Sender dashboard (`grafana/sender-dashboard.json`) previously queried the
production geyser ClickHouse directly: an `ARRAY JOIN` over
`geyser_transactions` joined to `tip_accounts`, over a 30-minute window, on
every (auto-refreshing) panel load. That scan is far too heavy to expose on a
public dashboard. `sender_provider_rollup.sql` defines a per-minute,
per-provider rollup (a materialized view writing into an
`AggregatingMergeTree` table) that summarizes tipping-transaction **count**,
**total tips**, **total fees**, and **median tip / fee** (as quantile
aggregate state). The dashboard now reads from the rollup table.

## Ownership — read before deploying

> **These objects are NOT created or managed by `rpc-latency-monitor`.**
> This repository never connects to or writes to the production geyser
> ClickHouse. The rollup lives in the geyser cluster (database `default`,
> next to `geyser_transactions` and `tip_accounts`) and **must be created
> and operated by the geyser-data team.**

This is a hard dependency: the Sender dashboard will return no data until the
geyser-data team has applied `sender_provider_rollup.sql` on the geyser
cluster. Hand-off checklist for that team:

1. Review `sender_provider_rollup.sql` and confirm column names/types match
   the current `geyser_transactions` / `tip_accounts` schema
   (`balance_deltas`, `fee_sol`, `is_vote`, `static_unsigned_writable_accounts`,
   `loaded_writable_accounts`, `tip_accounts.tip_account`, `tip_accounts.relayer`).
2. Create the target table and materialized view from the SQL.
3. (Optional) Run the commented-out backfill `INSERT ... SELECT` once, off-peak,
   to populate history; the materialized view only captures rows ingested after
   it is created.
4. Confirm the `TTL` (30 days) and partitioning are acceptable for the cluster.

## Files

| File                          | Purpose                                                        |
| ----------------------------- | -------------------------------------------------------------- |
| `sender_provider_rollup.sql`  | Target table + materialized view + optional backfill statement |

## Reading from the rollup

Counts and sums are exact. Medians are stored as `quantile(0.5)` aggregate
state and must be read with the matching `-Merge` combinator over the desired
window, e.g.:

```sql
SELECT
    provider,
    countMerge(tx_count)                              AS transactions,
    round(sumMerge(total_tips_lamports) / 1e9, 4)     AS total_tips_sol,
    round(sumMerge(total_fees_sol), 4)                AS total_fees_sol,
    round(quantileMerge(0.5)(tip_quantiles_lamports)) AS median_tip_lamports,
    round(quantileMerge(0.5)(fee_quantiles_sol) * 1e9) AS median_fee_lamports
FROM default.sender_provider_rollup
WHERE bucket >= now() - INTERVAL 30 MINUTE
GROUP BY provider
ORDER BY transactions DESC;
```
