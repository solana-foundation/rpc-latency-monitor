#!/usr/bin/env bash
set -euo pipefail

# One deploy entrypoint for every provider. gcp/aws go through terraform,
# latitude/tsw through ansible. PROVIDER picks which; `all` does the lot.
# Run under `doppler run` so the sub-deploys see secrets.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROVIDER="${PROVIDER:-${1:-all}}"
TARGET="${TARGET:-all}"
if [ "$TARGET" = "alerts" ]; then
  PROVIDER=gcp
fi

run_gcp() {
  : "${PROJECT:?}" "${GCP_TF_STATE_BUCKET:?}"
  TF_STATE_BUCKET="$GCP_TF_STATE_BUCKET" REGION="${REGION:-us-east4}" \
    TARGET="${TARGET:-all}" IMAGE_SHA="${IMAGE_SHA:-}" PROJECT="$PROJECT" \
    "$ROOT/gcp/deploy.sh"
}

run_aws() {
  : "${AWS_TF_STATE_BUCKET:?}"
  TF_STATE_BUCKET="$AWS_TF_STATE_BUCKET" STATE_REGION="${STATE_REGION:-us-east-1}" \
    IMAGE_SHA="${IMAGE_SHA:-}" "$ROOT/aws/deploy.sh"
}

# Bare metal deploys over SSH from the self-hosted runner using its own
# ~/.ssh key (authorized on the hosts). Groups with no active inventory hosts
# no-op loudly, so `all` stays safe while a provider is still being provisioned.
run_metal() {
  local group="$1"
  local count
  count=$( (cd "$ROOT/ansible" && ansible "$group" --list-hosts 2>/dev/null) \
    | sed -n 's/.*hosts (\([0-9]*\)).*/\1/p' )
  if [ "${count:-0}" = "0" ]; then
    echo "note: ${group} inventory has no hosts — nothing deployed"
    return 0
  fi
  if ! compgen -G "$HOME/.ssh/id_*" >/dev/null 2>&1; then
    echo "warning: no SSH key at ~/.ssh/id_* on this runner — bare-metal (${group}) SSH auth will fail" >&2
  fi
  LIMIT="$group" IMAGE_SHA="${IMAGE_SHA:-}" "$ROOT/ansible/deploy.sh"
}

case "$PROVIDER" in
  gcp)      run_gcp ;;
  aws)      run_aws ;;
  latitude) run_metal latitude ;;
  tsw)      run_metal tsw ;;
  all)      run_gcp; run_aws; run_metal latitude; run_metal tsw ;;
  *) echo "unknown provider: $PROVIDER (all|gcp|aws|latitude|tsw)" >&2; exit 1 ;;
esac

echo "deploy complete (provider=$PROVIDER)"
