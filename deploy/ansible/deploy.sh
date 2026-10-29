#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# PLAYBOOK=monitor.yml (rpc-monitor, needs IMAGE_SHA) or grpc-bench.yml (the
# gRPC benchmark, optional GRPC_BENCH_REF, then publishes its dashboards).
PLAYBOOK="${PLAYBOOK:-monitor.yml}"
IMAGE_SHA="${IMAGE_SHA:-}"
LIMIT="${LIMIT:-}"
GRPC_BENCH_REF="${GRPC_BENCH_REF:-}"

case "$PLAYBOOK" in
  monitor.yml)
    : "${IMAGE_SHA:?set IMAGE_SHA to an immutable build sha}"
    echo "deploying ref: $IMAGE_SHA"
    ;;
  grpc-bench.yml) ;;
  *) echo "unknown PLAYBOOK: $PLAYBOOK (monitor.yml|grpc-bench.yml)" >&2; exit 1 ;;
esac
: "${MONITOR_DOPPLER_TOKEN:?VM Doppler service token, set via Doppler}"
: "${KNOWN_HOSTS_B64:?base64 known_hosts covering every inventory host, set via Doppler}"

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
if [ -n "$GRPC_BENCH_REF" ]; then
  echo "grpc bench ref override: $GRPC_BENCH_REF"
  printf 'grpc_bench_ref: "%s"\n' "$GRPC_BENCH_REF" >>"$VARS_FILE"
fi

cd "$SCRIPT_DIR"
ansible-playbook "$PLAYBOOK" \
  ${LIMIT:+--limit "$LIMIT"} \
  --extra-vars "@${VARS_FILE}"

[ "$PLAYBOOK" = grpc-bench.yml ] || exit 0

# Publish the benchmark dashboards at the same ref the hosts now run, so panels
# never expect a schema the subscribers aren't writing yet. They live in their
# own `grpc-latency` folder on the solanafoundation stack only (rpc-latency's
# boards are separate), and query the `benchmark-clickhouse` datasource
# (geyser FRA ClickHouse, database benchmark).
BENCH_FOLDER_UID="${GRPC_BENCH_GRAFANA_FOLDER_UID:-grpc-latency}"
BENCH_REF="${GRPC_BENCH_REF:-$(sed -n 's/^grpc_bench_ref: "\([0-9a-f]*\)"$/\1/p' roles/grpc_bench/defaults/main.yml)}"
if [ -z "$BENCH_REF" ]; then
  echo "note: no grpc_bench_ref pinned — benchmark dashboards not published"
elif [ -z "${GRAFANA_API_URL:-}" ] || [ -z "${GRAFANA_API_TOKEN:-}" ]; then
  echo "warn: GRAFANA_API_URL/GRAFANA_API_TOKEN unset — benchmark dashboards not published"
else
  # Idempotent: 409/412 just means the folder already exists.
  code="$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$GRAFANA_API_URL/api/folders" \
    -H "Authorization: Bearer $GRAFANA_API_TOKEN" -H "Content-Type: application/json" \
    -d "{\"uid\":\"${BENCH_FOLDER_UID}\",\"title\":\"${BENCH_FOLDER_UID}\"}")"
  case "$code" in 200|409|412) ;; *) echo "error: creating Grafana folder ${BENCH_FOLDER_UID} returned HTTP $code" >&2; exit 1 ;; esac
  for name in solana-streaming-benchmark solana-streaming-head-to-head solana-streaming-overview; do
    src="https://raw.githubusercontent.com/solana-foundation/grpc-latency-monitor/${BENCH_REF}/grafana/${name}.json"
    curl -sS --fail-with-body "$src" \
      | jq --arg folder "$BENCH_FOLDER_UID" \
        '{dashboard: (. + {id: null}), folderUid: $folder, overwrite: true}' \
      | curl -sS --fail-with-body -X POST "$GRAFANA_API_URL/api/dashboards/db" \
        -H "Authorization: Bearer $GRAFANA_API_TOKEN" -H "Content-Type: application/json" \
        --data-binary @- >/dev/null
    echo "published dashboard ${name} @ ${BENCH_REF:0:7} -> folder ${BENCH_FOLDER_UID}"
  done
fi
