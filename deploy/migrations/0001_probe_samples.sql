-- BigQuery, project rpc-latency-monitor. Apply with:
--   bq query --use_legacy_sql=false --location=us-east4 --project_id=rpc-latency-monitor < deploy/migrations/0001_probe_samples.sql
-- Written by raw-api /ingest/samples (fleet batches), read by /raw/samples.

CREATE SCHEMA IF NOT EXISTS `rpc-latency-monitor.raw` OPTIONS (location = 'us-east4');

CREATE TABLE IF NOT EXISTS `rpc-latency-monitor.raw.probe_samples` (
  ts TIMESTAMP NOT NULL,
  provider STRING NOT NULL,
  method STRING NOT NULL,
  infra STRING NOT NULL,
  region STRING NOT NULL,
  target STRING,
  status STRING NOT NULL,
  error_kind STRING NOT NULL,
  latency_ms FLOAT64 NOT NULL,
  slot INT64
)
PARTITION BY DATE(ts)
CLUSTER BY provider, method
OPTIONS (partition_expiration_days = 400);
