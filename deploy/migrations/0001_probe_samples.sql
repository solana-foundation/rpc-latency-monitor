-- Cloud SQL Postgres, instance rpc-raw-samples, database samples. Apply with:
--   gcloud sql connect rpc-raw-samples --project=rpc-latency-monitor --database=samples --user=rawapi < deploy/migrations/0001_probe_samples.sql
-- Written by raw-api /ingest/samples (fleet batches), read by /raw/samples.
-- Rows older than 400 days are purged by raw-api's daily retention task.

CREATE TABLE IF NOT EXISTS probe_samples (
  ts timestamptz NOT NULL,
  provider text NOT NULL,
  method text NOT NULL,
  infra text NOT NULL,
  region text NOT NULL,
  target text NOT NULL DEFAULT '',
  status text NOT NULL,
  error_kind text NOT NULL,
  latency_ms double precision NOT NULL,
  slot bigint
);

CREATE INDEX IF NOT EXISTS probe_samples_ts ON probe_samples USING brin (ts);
CREATE INDEX IF NOT EXISTS probe_samples_provider_method_ts
  ON probe_samples (provider, method, ts DESC);
