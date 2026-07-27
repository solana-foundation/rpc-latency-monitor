# Deploy & operations

Everything needed to run the fleet lives in this repo (migrated from the
private `rpc-latency-monitor-ops` repo for transparency). The same neutrality
rule applies to operations as to measurement: anyone can read exactly how the
fleet is deployed, configured, and alerted.

## Layout

- `deploy.sh` — single entrypoint; `PROVIDER=all|gcp|aws|latitude|tsw` routes
  clouds to terraform and bare metal to ansible.
- `gcp/` — per-region Container-Optimized-OS VM fleet (terraform), startup
  script, and `config.yaml`: the fleet runtime config shipped to every box on
  every infra.
- `aws/` — per-region EC2 fleet (terraform), rolled one region at a time.
- `ansible/` — bare-metal fleets (native systemd, no docker), built from the
  pinned git sha. Inventories are **not committed** (they hold box IPs / SSH
  targets); `deploy.sh` materializes them from Doppler
  (`INVENTORY_LATITUDE_B64` / `INVENTORY_TSW_B64`, base64 of a file shaped
  like `inventory/example.yml.tmpl`).
- `shared/run-monitor.sh` — the container-run core shared by GCP and AWS.
- `cloudflare/waf-rules.json` — WAF/rate-limit rules for the public surface.
- `../grafana/alloy-config.alloy` — local scrape + authenticated remote_write.
- `../grafana/alerts/monitor-data.json` — provisioned alert rules.
- `Dockerfile`, `cloudbuild.yaml`, `docker-compose.yaml`, `prometheus.yml`,
  `grafana/` — image build and the local dev stack.

## Pipeline

Merge to `main` → **Build & Publish** (fmt/clippy/test → Cloud Build →
Artifact Registry → push dashboards) → **Deploy** fires via `workflow_run`
(terraform apply + staggered VM resets + ansible + alert push). Manual deploys:
Actions → Deploy → Run workflow (`target`: all|fleet|alerts, `provider`:
all|gcp|aws|latitude|tsw, `image_sha`: pin a build or blank to redeploy state).
Only immutable image shas deploy — never `:latest`.

## Secrets model (nothing secret is committed)

- **GitHub vars:** `GCP_WORKLOAD_IDENTITY_PROVIDER`, `GCP_SERVICE_ACCOUNT`,
  `GCP_PROJECT_ID`, `TF_STATE_BUCKET`, `AWS_TF_STATE_BUCKET`,
  `AWS_DEPLOY_ROLE_ARN`, `GRAFANA_API_URL`, `GRAFANA_FOLDER_UID`.
- **GitHub secrets (environment `prod`):** `DOPPLER_TOKEN`, `GRAFANA_API_TOKEN`.
- **Doppler (`rpc-latency-monitor/prd`):** provider keys,
  `MONITOR_DOPPLER_TOKEN` (VM service token), `REFERENCE_RPC_URL` (the
  reference node — deliberately unlisted), `INVENTORY_LATITUDE_B64`,
  `INVENTORY_TSW_B64`, `SLACK_DEPLOY_WEBHOOK_URL` (deploy notifications,
  fetched by the workflow at run time), `GRAFANA_API_*`,
  `GRAFANA_CLOUD_PROM_*`.
- Cloud auth is Workload Identity Federation / OIDC — no static cloud keys
  anywhere. VM secrets are fetched from Doppler at boot, never baked into
  images or metadata beyond the scoped service token.

`${ENV_VAR}` placeholders in `config.yaml` (provider URLs,
`reference_slot.endpoint`, `reference_check.rpc_url`, `gpa_derive.endpoint`)
are resolved from the environment at startup.

## Self-hosted runner (public-repo hardening)

The Deploy workflow runs on a self-hosted runner (`rpc-latency` label).
Because this repo is public, the following must stay true:

- Deploy triggers are `workflow_dispatch` and `workflow_run` only — fork PRs
  cannot fire them, and `pull_request` workflows (CI) run exclusively on
  GitHub-hosted runners.
- Deploy-path secrets live in the `prod` environment, which is restricted to
  `main`.
- Repo Actions settings require approval for outside collaborators' workflow
  runs.
