#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TF_DIR="$SCRIPT_DIR/terraform"

PROJECT="${PROJECT:?PROJECT=<gcp project id>}"
REGION="${REGION:-us-east4}"
TARGET="${TARGET:-all}"
IMAGE_SHA="${IMAGE_SHA:-}"

push_alerts() {
  : "${GRAFANA_API_URL:?set via Doppler}"
  : "${GRAFANA_API_TOKEN:?set via Doppler}"
  echo "::add-mask::$GRAFANA_API_TOKEN" 2>/dev/null || true
  local ds_uid="${GRAFANA_DATASOURCE_UID:-grafanacloud-prom}"
  local folder_uid="${GRAFANA_FOLDER_UID:-}"
  local f="$REPO_ROOT/grafana/alerts/monitor-data.json"
  local resolved group payload body code
  resolved="$(jq --arg ds "$ds_uid" --arg folder "$folder_uid" \
    'walk(if type=="string" then gsub("\\$\\{GRAFANA_DATASOURCE_UID\\}";$ds)|gsub("\\$\\{GRAFANA_FOLDER_UID\\}";$folder) else . end)' "$f")"
  group="$(jq -r '.groups[0].name' <<<"$resolved")"
  payload="$(jq '.groups[0]|{title:.name,interval:60,rules:.rules}' <<<"$resolved")"
  body="$(curl -sS -w '\n%{http_code}' -X PUT \
    "$GRAFANA_API_URL/api/v1/provisioning/folder/$folder_uid/rule-groups/$group" \
    -H "Authorization: Bearer $GRAFANA_API_TOKEN" -H "Content-Type: application/json" \
    -H "X-Disable-Provenance: true" -d "$payload")"
  code="${body##*$'\n'}"; body="${body%$'\n'*}"
  [ "$code" -ge 200 ] && [ "$code" -lt 300 ] || { echo "alert upsert failed (HTTP $code): $body" >&2; exit 1; }
  echo "pushed alert group $group"
}

deploy_fleet() {
  : "${TF_STATE_BUCKET:?set the GCS bucket holding terraform state}"
  : "${MONITOR_DOPPLER_TOKEN:?VM Doppler service token, set via Doppler}"
  terraform -chdir="$TF_DIR" init -reconfigure \
    -backend-config="bucket=${TF_STATE_BUCKET}" -backend-config="prefix=rpc-latency-monitor"

  local image
  if [ -n "$IMAGE_SHA" ]; then
    image="${REGION}-docker.pkg.dev/${PROJECT}/rpc-latency-monitor/rpc-latency-monitor:${IMAGE_SHA}"
  else
    image="$(terraform -chdir="$TF_DIR" output -raw monitor_image 2>/dev/null || true)"
  fi
  case "$image" in
    ""|*:latest)
      echo "refusing to deploy: need an immutable image (set IMAGE_SHA; no :latest, no prior image in state)" >&2
      exit 1 ;;
  esac
  echo "deploying image: $image"

  TF_VAR_doppler_token="${MONITOR_DOPPLER_TOKEN}" \
    terraform -chdir="$TF_DIR" apply -auto-approve \
      -var "project_id=${PROJECT}" -var "monitor_image=${image}"

  RESET_DELAY="${RESET_DELAY:-75}"
  first=1
  gcloud compute instances list --project "$PROJECT" \
    --filter="labels.service=rpc-latency-monitor" --format="value(name,zone)" |
    while read -r name zone; do
      [ -z "$name" ] && continue
      [ "$first" -eq 1 ] || sleep "$RESET_DELAY"
      first=0
      gcloud compute instances reset "$name" --project "$PROJECT" --zone "$zone"
      echo "reset $name"
    done
}

RAW_API_PATHS="src/raw_api.rs src/bin/raw-api.rs Cargo.toml Cargo.lock deploy/Dockerfile"

deploy_raw_api() {
  : "${RAW_API_JWT_SECRET:?set via Doppler}"
  : "${RAW_API_GRAFANA_TOKEN:?set via Doppler}"
  : "${GRAFANA_API_URL:?set via Doppler}"
  local deployed image
  deployed="$(gcloud run services describe rpc-raw-api --project "$PROJECT" --region "$REGION" \
    --format 'value(spec.template.spec.containers[0].image)' 2>/dev/null || true)"

  if [ "$TARGET" = "all" ]; then
    if [ -z "$IMAGE_SHA" ]; then
      echo "raw-api: no IMAGE_SHA on a full deploy — leaving the current revision in place"
      return 0
    fi
    local deployed_sha="${deployed##*:}"
    if [ -n "$deployed_sha" ] && [ "$deployed_sha" != "latest" ] \
      && git cat-file -e "${deployed_sha}^{commit}" 2>/dev/null \
      && git cat-file -e "${IMAGE_SHA}^{commit}" 2>/dev/null; then
      # shellcheck disable=SC2086
      if ! git diff --name-only "$deployed_sha" "$IMAGE_SHA" -- $RAW_API_PATHS | grep -q .; then
        echo "raw-api unchanged between $deployed_sha and $IMAGE_SHA — skipping (dispatch target=rawapi to force)"
        return 0
      fi
    fi
  fi

  if [ -n "$IMAGE_SHA" ]; then
    image="${REGION}-docker.pkg.dev/${PROJECT}/rpc-latency-monitor/rpc-latency-monitor:${IMAGE_SHA}"
  else
    image="$deployed"
  fi
  case "$image" in
    ""|*:latest)
      echo "refusing to deploy raw-api: need an immutable image (set IMAGE_SHA; no :latest, no prior revision)" >&2
      exit 1 ;;
  esac
  echo "deploying rpc-raw-api image: $image"
  gcloud run deploy rpc-raw-api --project "$PROJECT" --region "$REGION" \
    --image "$image" --command raw-api --port 8080 \
    --min-instances 0 --max-instances 1 --cpu 1 --memory 256Mi \
    --allow-unauthenticated --quiet \
    --set-env-vars "^@^GRAFANA_API_URL=${GRAFANA_API_URL}@GRAFANA_DATASOURCE_UID=${GRAFANA_DATASOURCE_UID:-grafanacloud-prom}@RAW_API_JWT_SECRET=${RAW_API_JWT_SECRET}@RAW_API_GRAFANA_TOKEN=${RAW_API_GRAFANA_TOKEN}"
}

case "$TARGET" in
  alerts) push_alerts ;;
  fleet) deploy_fleet ;;
  rawapi) deploy_raw_api ;;
  all) push_alerts; deploy_fleet; deploy_raw_api ;;
  *) echo "unknown TARGET: $TARGET (want: all|fleet|alerts|rawapi)" >&2; exit 1 ;;
esac

echo "ops deploy complete (target=$TARGET)"
