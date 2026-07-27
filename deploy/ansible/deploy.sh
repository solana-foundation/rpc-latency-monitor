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
# (see inventory/example.yml.tmpl for the shape). Doppler is authoritative:
# when the env var is set it always overwrites, so updates (new boxes, rotated
# keys) land on the next deploy even on a persistent runner. A hand-written
# local file is only used when the env var is absent (local runs).
materialize() {
  local var="$1" file="$SCRIPT_DIR/$2"
  local value="${!var:-}"
  if [ -z "$value" ]; then
    [ -f "$file" ] && echo "note: $2 not in $var — using existing local file"
    return 0
  fi
  echo "$value" | base64 -d >"$file"
  chmod 600 "$file"
  echo "materialized $2 from $var"
}
materialize INVENTORY_LATITUDE_B64 inventory/latitude.yml
materialize INVENTORY_TSW_B64 inventory/teraswitch.yml

# Host keys (base64 of an ssh known_hosts covering every inventory host) back
# the strict host-key check in ansible.cfg. Refreshed in Doppler by
# ssh-keyscan whenever a box is provisioned or reinstalled.
materialize KNOWN_HOSTS_B64 known_hosts
if [ ! -f "$SCRIPT_DIR/known_hosts" ]; then
  echo "error: no known_hosts — set KNOWN_HOSTS_B64 (strict host-key checking needs it)" >&2
  exit 1
fi

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
