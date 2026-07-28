#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

IMAGE_SHA="${IMAGE_SHA:-}"
LIMIT="${LIMIT:-}"

: "${IMAGE_SHA:?set IMAGE_SHA to an immutable build sha}"
: "${MONITOR_DOPPLER_TOKEN:?VM Doppler service token, set via Doppler}"
: "${KNOWN_HOSTS_B64:?base64 known_hosts covering every inventory host, set via Doppler}"

echo "deploying ref: $IMAGE_SHA"

materialize() {
  local var="$1" file="$SCRIPT_DIR/$2"
  local value="${!var:-}"
  if [ -z "$value" ]; then
    [ -f "$file" ] && echo "note: $2 not in $var — using existing local file"
    return 0
  fi
  mkdir -p "$(dirname "$file")"
  echo "$value" | base64 -d >"$file"
  chmod 600 "$file"
  echo "materialized $2 from $var"
}
materialize INVENTORY_LATITUDE_B64 inventory/latitude.yml
materialize INVENTORY_TSW_B64 inventory/teraswitch.yml
materialize KNOWN_HOSTS_B64 known_hosts

if [ -n "$LIMIT" ]; then
  count=$( (cd "$SCRIPT_DIR" && ansible "$LIMIT" --list-hosts 2>/dev/null) \
    | sed -n 's/.*hosts (\([0-9]*\)).*/\1/p' )
  if [ "${count:-0}" = "0" ]; then
    echo "note: ${LIMIT} has no inventory hosts — nothing deployed"
    exit 0
  fi
fi

VARS_FILE="$(mktemp)"
chmod 600 "$VARS_FILE"
trap 'rm -f "$VARS_FILE"' EXIT
printf 'monitor_ref: "%s"\ndoppler_token: "%s"\n' "$IMAGE_SHA" "$MONITOR_DOPPLER_TOKEN" >"$VARS_FILE"

cd "$SCRIPT_DIR"
ansible-playbook site.yml \
  ${LIMIT:+--limit "$LIMIT"} \
  --extra-vars "@${VARS_FILE}"
