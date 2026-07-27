#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Bare metal builds the monitor natively (no docker) from the pinned git ref —
# same immutable sha the docker fleets pin their image to.
IMAGE_SHA="${IMAGE_SHA:-}"
# LIMIT lets you target one provider/host, e.g. LIMIT=tsw or LIMIT=rpc-monitor-fra-1.
LIMIT="${LIMIT:-}"

: "${IMAGE_SHA:?set IMAGE_SHA to an immutable build sha}"
: "${MONITOR_DOPPLER_TOKEN:?VM Doppler service token, set via Doppler}"

echo "deploying ref: $IMAGE_SHA"

# Inventories carry the bare-metal box IPs (SSH targets) and are not committed
# to this public repo. They are materialized from Doppler at deploy time:
# INVENTORY_LATITUDE_B64 / INVENTORY_TSW_B64 = base64 of the inventory YAML
# (see inventory/example.yml.tmpl for the shape). A pre-existing local file is
# left alone so local runs with a hand-written inventory still work.
materialize_inventory() {
  local var="$1" file="$SCRIPT_DIR/inventory/$2"
  local value="${!var:-}"
  [ -f "$file" ] && return 0
  [ -z "$value" ] && return 0
  echo "$value" | base64 -d >"$file"
  chmod 600 "$file"
  echo "materialized inventory/$2 from $var"
}
materialize_inventory INVENTORY_LATITUDE_B64 latitude.yml
materialize_inventory INVENTORY_TSW_B64 teraswitch.yml

# Pass vars via a locked-down temp file, not inline -e, so the Doppler token
# never appears in `ps aux` on the runner.
VARS_FILE="$(mktemp)"
chmod 600 "$VARS_FILE"
trap 'rm -f "$VARS_FILE"' EXIT
printf 'monitor_ref: "%s"\ndoppler_token: "%s"\n' "$IMAGE_SHA" "$MONITOR_DOPPLER_TOKEN" >"$VARS_FILE"

cd "$SCRIPT_DIR"
ansible-playbook site.yml \
  ${LIMIT:+--limit "$LIMIT"} \
  --extra-vars "@${VARS_FILE}"
