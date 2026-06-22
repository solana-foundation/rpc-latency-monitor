-- Sender provider pre-aggregation rollup
-- =====================================================================
-- Purpose
--   The Grafana "Sender" dashboard (grafana/sender-dashboard.json) used to
--   query the raw production geyser tables directly, doing an
--   ARRAY JOIN over geyser_transactions x tip_accounts on every refresh.
--   That scan is far too heavy to expose on a public, auto-refreshing
--   dashboard. This file defines a small per-minute, per-provider rollup
--   so the dashboard can read pre-aggregated rows instead.
--
-- IMPORTANT — OWNERSHIP / DEPLOYMENT
--   This SQL is NOT applied automatically by rpc-latency-monitor and MUST
--   NOT be run against production by this repo. The objects below live in
--   the geyser ClickHouse cluster (database `default`, alongside
--   geyser_transactions / tip_accounts) and must be created by the
--   geyser-data team. See clickhouse/README.md for the explicit hand-off.
--
-- Granularity
--   1-minute buckets keyed on (provider, bucket). The dashboard's default
--   window is 30 minutes, so this keeps per-panel reads to a few dozen
--   rows per provider while still supporting Grafana's $__timeInterval
--   re-bucketing for the time series panel.
--
-- Medians
--   Total tips/fees and counts are exact (SummingMergeTree-style sums via
--   AggregatingMergeTree). Medians are stored as quantile aggregate state
--   so the dashboard can compute an (approximate) median over an arbitrary
--   window by merging states, rather than over a single bucket.
-- =====================================================================

-- ---------------------------------------------------------------------
-- Target rollup table
-- ---------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS default.sender_provider_rollup
(
    bucket            DateTime,                              -- 1-minute bucket (UTC), from gt.ingested_at
    provider          LowCardinality(String),               -- ta.relayer
    tx_count          AggregateFunction(count),             -- tipping, non-vote tx count
    total_tips_lamports  AggregateFunction(sum, UInt64),    -- sum of positive tip balance deltas (lamports)
    total_fees_sol       AggregateFunction(sum, Float64),   -- sum of gt.fee_sol
    tip_quantiles_lamports AggregateFunction(quantile(0.5), UInt64),  -- median tip state (lamports)
    fee_quantiles_sol      AggregateFunction(quantile(0.5), Float64)  -- median fee state (SOL)
)
ENGINE = AggregatingMergeTree
PARTITION BY toYYYYMMDD(bucket)
ORDER BY (provider, bucket)
TTL bucket + INTERVAL 30 DAY;

-- ---------------------------------------------------------------------
-- Materialized view that populates the rollup as rows land in
-- geyser_transactions. One source row fans out across its writable
-- accounts (ARRAY JOIN) and is matched to a tip account / relayer, then
-- collapsed back to one aggregate row per (provider, minute).
-- ---------------------------------------------------------------------
CREATE MATERIALIZED VIEW IF NOT EXISTS default.sender_provider_rollup_mv
TO default.sender_provider_rollup
AS
SELECT
    toStartOfMinute(gt.ingested_at)                         AS bucket,
    ta.relayer                                              AS provider,
    countState()                                            AS tx_count,
    sumState(toUInt64(gt.balance_deltas[acct]))            AS total_tips_lamports,
    sumState(gt.fee_sol)                                    AS total_fees_sol,
    quantileState(0.5)(toUInt64(gt.balance_deltas[acct]))  AS tip_quantiles_lamports,
    quantileState(0.5)(gt.fee_sol)                          AS fee_quantiles_sol
FROM default.geyser_transactions AS gt
ARRAY JOIN arrayConcat(gt.static_unsigned_writable_accounts, gt.loaded_writable_accounts) AS acct
INNER JOIN default.tip_accounts AS ta ON ta.tip_account = acct
WHERE gt.is_vote = 0
  AND gt.balance_deltas[acct] > 0
GROUP BY bucket, provider;

-- ---------------------------------------------------------------------
-- OPTIONAL backfill (run once, manually, by the geyser-data team).
-- The materialized view only captures rows ingested AFTER it is created.
-- To populate history (e.g. so the 30-day TTL window is full on day one),
-- run an INSERT SELECT over the existing range. This is a heavy scan over
-- the raw tables — run off-peak and bound the time range explicitly.
--
-- INSERT INTO default.sender_provider_rollup
-- SELECT
--     toStartOfMinute(gt.ingested_at)                         AS bucket,
--     ta.relayer                                              AS provider,
--     countState()                                            AS tx_count,
--     sumState(toUInt64(gt.balance_deltas[acct]))            AS total_tips_lamports,
--     sumState(gt.fee_sol)                                    AS total_fees_sol,
--     quantileState(0.5)(toUInt64(gt.balance_deltas[acct]))  AS tip_quantiles_lamports,
--     quantileState(0.5)(gt.fee_sol)                          AS fee_quantiles_sol
-- FROM default.geyser_transactions AS gt
-- ARRAY JOIN arrayConcat(gt.static_unsigned_writable_accounts, gt.loaded_writable_accounts) AS acct
-- INNER JOIN default.tip_accounts AS ta ON ta.tip_account = acct
-- WHERE gt.is_vote = 0
--   AND gt.balance_deltas[acct] > 0
--   AND gt.ingested_at >= now() - INTERVAL 30 DAY
-- GROUP BY bucket, provider;
