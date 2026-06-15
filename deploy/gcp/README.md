# GCP deployment

One Container-Optimized OS VM per region runs the monitor plus a Grafana Alloy agent that
`remote_write`s to the public Grafana Cloud stack. Each VM is tagged with its region.

## Prerequisites

- A GCP project (`project_id`) and `gcloud auth application-default login`.
- The monitor image pushed to a registry (`monitor_image`).
- A Doppler service token whose secrets include the provider api-keys and the `GRAFANA_CLOUD_*`
  remote_write credentials. Only this token is passed to Terraform — individual secrets are pulled
  on the VM at boot.
- `config.yaml` at the repo root. Set `server.bind: 127.0.0.1:9464` so the metrics endpoint is only
  reachable by the local Alloy agent.

## Deploy

```bash
cd deploy/gcp/terraform
cp terraform.tfvars.example terraform.tfvars   # edit project_id, monitor_image, doppler_token, locations
terraform init
terraform apply
```

Add or remove regions by editing the `locations` map (region label → zone).
