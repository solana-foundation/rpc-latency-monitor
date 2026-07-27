#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TF_DIR="$SCRIPT_DIR/terraform"

# Immutable image only. IMAGE_SHA (from the build-publish dispatch, or a manual
# input) pins an exact build; blank means redeploy whatever is already in state.
# The image lives in the (public) GCP Artifact Registry; AWS pulls it anonymously.
IMAGE_SHA="${IMAGE_SHA:-}"
IMAGE_REPO="${IMAGE_REPO:-us-east4-docker.pkg.dev/rpc-latency-monitor/rpc-latency-monitor/rpc-latency-monitor}"
STATE_REGION="${STATE_REGION:-us-east-1}"

: "${TF_STATE_BUCKET:?set the S3 bucket holding terraform state}"
: "${MONITOR_DOPPLER_TOKEN:?VM Doppler service token, set via Doppler}"

terraform -chdir="$TF_DIR" init -reconfigure \
  -backend-config="bucket=${TF_STATE_BUCKET}" \
  -backend-config="key=rpc-latency-monitor/aws.tfstate" \
  -backend-config="region=${STATE_REGION}" \
  -backend-config="use_lockfile=true"

if [ -n "$IMAGE_SHA" ]; then
  image="${IMAGE_REPO}:${IMAGE_SHA}"
else
  image="$(terraform -chdir="$TF_DIR" output -raw monitor_image 2>/dev/null || true)"
fi
case "$image" in
  ""|*:latest)
    echo "refusing to deploy: need an immutable image (set IMAGE_SHA; no :latest, no prior image in state)" >&2
    exit 1 ;;
esac
echo "deploying image: $image"

export TF_VAR_doppler_token="${MONITOR_DOPPLER_TOKEN}"

# Roll one region at a time (each is replaced via user_data_replace_on_change),
# so the AWS vantage is never fully dark during a redeploy — mirrors the GCP
# staggered reset. A final untargeted apply reconciles shared/IAM resources.
MODULES="us_east_1 us_west_1 eu_west_2 eu_central_1 eu_west_1 ap_northeast_1 ap_southeast_1"
RESET_DELAY="${RESET_DELAY:-30}"
first=1
for m in $MODULES; do
  [ "$first" -eq 1 ] || sleep "$RESET_DELAY"
  first=0
  terraform -chdir="$TF_DIR" apply -auto-approve -refresh=false \
    -target="module.${m}" -var "monitor_image=${image}"
  echo "rolled module.${m}"
done

terraform -chdir="$TF_DIR" apply -auto-approve -var "monitor_image=${image}"

echo "aws deploy complete"
