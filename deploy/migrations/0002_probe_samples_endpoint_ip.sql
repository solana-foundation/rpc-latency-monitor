-- Apply with:
--   gcloud sql connect rpc-raw-samples --project=rpc-latency-monitor --database=samples --user=rawapi < deploy/migrations/0002_probe_samples_endpoint_ip.sql

ALTER TABLE probe_samples ADD COLUMN IF NOT EXISTS endpoint_ip inet;
