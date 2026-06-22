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

push_dashboards() {
  : "${GRAFANA_API_URL:?set via Doppler}"
  : "${GRAFANA_API_TOKEN:?set via Doppler}"
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

# Upsert Grafana alert rules (alerting as code) from grafana/alerts/*.json.
# Placeholders in the JSON are substituted from existing Grafana env vars; no new
# secrets are introduced. ${GRAFANA_DATASOURCE_UID} defaults to "prometheus".
push_alerts() {
  : "${GRAFANA_API_URL:?set via Doppler}"
  : "${GRAFANA_API_TOKEN:?set via Doppler}"
  echo "::add-mask::$GRAFANA_API_TOKEN" 2>/dev/null || true
  local ds_uid="${GRAFANA_DATASOURCE_UID:-prometheus}"
  local folder_uid="${GRAFANA_FOLDER_UID:-}"
  for f in "$REPO_ROOT"/grafana/alerts/*.json; do
    [ -e "$f" ] || continue
    local uid payload
    uid="$(jq -r '.uid' "$f")"
    payload="$(jq \
      --arg ds "$ds_uid" \
      --arg folder "$folder_uid" \
      'walk(if type == "string" then
              gsub("\\$\\{GRAFANA_DATASOURCE_UID\\}"; $ds)
              | gsub("\\$\\{GRAFANA_FOLDER_UID\\}"; $folder)
            else . end)' "$f")"
    # Upsert: update by UID, falling back to create only when the rule does not
    # exist yet (404). Any other PUT failure (400/401/403/5xx) is surfaced and
    # hard-fails so a transient error can't silently create a duplicate rule.
    local put_body put_code
    put_body="$(curl -sS -w '\n%{http_code}' -X PUT "$GRAFANA_API_URL/api/v1/provisioning/alert-rules/$uid" \
      -H "Authorization: Bearer $GRAFANA_API_TOKEN" \
      -H "Content-Type: application/json" \
      -H "X-Disable-Provenance: true" \
      -d "$payload")"
    put_code="${put_body##*$'\n'}"
    put_body="${put_body%$'\n'*}"
    if [ "$put_code" = "404" ]; then
      curl -sS --fail-with-body -X POST "$GRAFANA_API_URL/api/v1/provisioning/alert-rules" \
        -H "Authorization: Bearer $GRAFANA_API_TOKEN" \
        -H "Content-Type: application/json" \
        -H "X-Disable-Provenance: true" \
        -d "$payload" >/dev/null
    elif [ "$put_code" -lt 200 ] || [ "$put_code" -ge 300 ]; then
      echo "failed to upsert alert rule $(basename "$f") (HTTP $put_code): $put_body" >&2
      exit 1
    fi
    echo "pushed alert rule $(basename "$f")"
  done
}

deploy_gcp() {
  : "${TF_STATE_BUCKET:?set the GCS bucket holding terraform state}"
  : "${MONITOR_DOPPLER_TOKEN:?VM Doppler service token, set via Doppler}"

  gcloud builds submit "$REPO_ROOT" --project "$PROJECT" \
    --config "$REPO_ROOT/deploy/cloudbuild.yaml" \
    --substitutions=_IMAGE="$IMAGE" \
    --suppress-logs

  terraform -chdir="$TF_DIR" init -reconfigure \
    -backend-config="bucket=${TF_STATE_BUCKET}" \
    -backend-config="prefix=rpc-latency-monitor"
  TF_VAR_doppler_token="${MONITOR_DOPPLER_TOKEN}" \
    terraform -chdir="$TF_DIR" apply -auto-approve \
      -var "project_id=${PROJECT}" \
      -var "monitor_image=${IMAGE}"

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
  grafana) push_dashboards; push_alerts ;;
  gcp) deploy_gcp ;;
  all) push_dashboards; push_alerts; deploy_gcp ;;
  *) echo "unknown TARGET: $TARGET (want: all|gcp|grafana)" >&2; exit 1 ;;
esac

echo "Deployed $IMAGE (target=$TARGET)"
