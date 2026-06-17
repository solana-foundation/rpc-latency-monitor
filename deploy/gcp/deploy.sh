#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TF_DIR="$SCRIPT_DIR/terraform"

PROJECT="${PROJECT:?PROJECT=<gcp project id>}"
REGION="${REGION:-us-east4}"
TARGET="${TARGET:-all}"

git_tag() {
  local sha dirty
  sha="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo manual)"
  git -C "$REPO_ROOT" diff --quiet 2>/dev/null || dirty="-dirty"
  printf '%s%s' "$sha" "${dirty:-}"
}

IMAGE="${REGION}-docker.pkg.dev/${PROJECT}/rpc-latency-monitor/rpc-latency-monitor:$(git_tag)"

# Upsert every dashboard in grafana/ into the Grafana Cloud stack. Creds come
# from Doppler; the API token is masked so it never lands in CI logs.
push_dashboards() {
  : "${GRAFANA_API_URL:?set via Doppler}"
  : "${GRAFANA_API_TOKEN:?set via Doppler}"
  # Folder UID is optional: empty puts dashboards in the root folder.
  echo "::add-mask::$GRAFANA_API_TOKEN" 2>/dev/null || true
  for f in "$REPO_ROOT"/grafana/*.json; do
    payload="$(jq -n --slurpfile d "$f" --arg folder "${GRAFANA_FOLDER_UID:-}" \
      '{dashboard: ($d[0] + {id: null}), folderUid: $folder, overwrite: true}')"
    curl -sS --fail-with-body -X POST "$GRAFANA_API_URL/api/dashboards/db" \
      -H "Authorization: Bearer $GRAFANA_API_TOKEN" \
      -H "Content-Type: application/json" \
      -d "$payload" >/dev/null
    echo "pushed dashboard $(basename "$f")"
  done
}

# Build the image, apply Terraform (which refreshes image + config metadata),
# then reset each VM so the startup script re-pulls and restarts the containers.
deploy_gcp() {
  : "${TF_STATE_BUCKET:?set the GCS bucket holding terraform state}"
  : "${MONITOR_DOPPLER_TOKEN:?VM Doppler service token, set via Doppler}"

  gcloud builds submit "$REPO_ROOT" --project "$PROJECT" \
    --config "$REPO_ROOT/deploy/cloudbuild.yaml" \
    --substitutions=_IMAGE="$IMAGE"

  terraform -chdir="$TF_DIR" init -reconfigure \
    -backend-config="bucket=${TF_STATE_BUCKET}" \
    -backend-config="prefix=rpc-latency-monitor"
  # Pass the token via TF_VAR_ (env), not -var, so it never lands in the
  # terraform process's /proc/<pid>/cmdline.
  TF_VAR_doppler_token="${MONITOR_DOPPLER_TOKEN}" \
    terraform -chdir="$TF_DIR" apply -auto-approve \
      -var "project_id=${PROJECT}" \
      -var "monitor_image=${IMAGE}"

  # Reset one region at a time with a delay between, so only one VM is ever in
  # its restart gap (COS reboot + image pull) — avoids a fleet-wide blind spot.
  RESET_DELAY="${RESET_DELAY:-75}"
  first=1
  gcloud compute instances list --project "$PROJECT" \
    --filter="labels.service=rpc-latency-monitor" \
    --format="value(name,zone)" |
    while read -r name zone; do
      [ -z "$name" ] && continue
      [ "$first" -eq 1 ] || sleep "$RESET_DELAY"
      first=0
      gcloud compute instances reset "$name" --project "$PROJECT" --zone "$zone"
      echo "reset $name"
    done
}

case "$TARGET" in
  grafana) push_dashboards ;;
  gcp) deploy_gcp ;;
  all) push_dashboards; deploy_gcp ;;
  *) echo "unknown TARGET: $TARGET (want: all|gcp|grafana)" >&2; exit 1 ;;
esac

echo "Deployed $IMAGE (target=$TARGET)"
