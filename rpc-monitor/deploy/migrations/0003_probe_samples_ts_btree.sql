-- Apply with:
--   gcloud sql connect rpc-raw-samples --project=rpc-latency-monitor --database=samples --user=rawapi < deploy/migrations/0003_probe_samples_ts_btree.sql
-- /raw/samples reads a ts range newest first. The BRIN index can't return rows
-- in ts order, so any filter set other than provider + method sorted the whole
-- range. CONCURRENTLY keeps /ingest/samples writing during the build; it can't
-- run inside a transaction, so keep this file to this one statement. An
-- interrupted build leaves an invalid index, which raw-api drops before retrying.
CREATE INDEX CONCURRENTLY IF NOT EXISTS probe_samples_ts_desc ON probe_samples (ts DESC);
