retry docker pull "$IMAGE"

ENV_FILE=/run/rpc-latency-monitor.env
fetch_env() {
  docker run --rm -e DOPPLER_TOKEN="$DOPPLER_TOKEN" \
    dopplerhq/cli:3@sha256:9f5f13bccb6856ca9a9807bddd5738e4edefb7c307ecbe515087f3e1f11ff4cd \
    secrets download --no-file --format docker >"$ENV_FILE"
}
retry fetch_env
chmod 600 "$ENV_FILE"

docker run -d --name rpc-monitor --network host --restart always \
  --memory 2g --memory-swap 2g \
  -e MONITOR_REGION="$REGION" -e MONITOR_INFRA="$INFRA" -e RUST_LOG=info \
  --env-file "$ENV_FILE" \
  -v "$CONF_DIR":/conf:ro \
  "$IMAGE" --config /conf/config.yaml

# Static host facts for Alloy's textfile collector: what this box costs per
# month (list price from terraform; see monthly_cost_usd) and its plan.
TEXTFILE_DIR="$CONF_DIR/textfile"
mkdir -p "$TEXTFILE_DIR"
cat >"$TEXTFILE_DIR/host_cost.prom.tmp" <<EOM
# HELP rpc_monitor_host_monthly_cost_usd Monthly cost of this host in USD (source=list-price until replaced by an invoice figure).
# TYPE rpc_monitor_host_monthly_cost_usd gauge
rpc_monitor_host_monthly_cost_usd{plan="${INSTANCE_PLAN:-unknown}",source="${COST_SOURCE:-list-price}"} ${MONTHLY_COST_USD:-0}
EOM
mv "$TEXTFILE_DIR/host_cost.prom.tmp" "$TEXTFILE_DIR/host_cost.prom"

# The host's /proc, /sys and / are mounted read-only so Alloy's unix exporter
# reports the VM, not the container (see prometheus.exporter.unix in config.alloy).
docker run -d --name alloy --network host --restart always \
  -e MONITOR_INFRA="$INFRA" -e MONITOR_HOST="$(hostname)" -e MONITOR_REGION="$REGION" \
  -e HOST_PROCFS=/host/proc -e HOST_SYSFS=/host/sys -e HOST_ROOTFS=/host/root \
  -e HOST_TEXTFILE_DIR=/conf/textfile \
  --env-file "$ENV_FILE" \
  -v "$CONF_DIR":/conf:ro \
  -v /proc:/host/proc:ro -v /sys:/host/sys:ro -v /:/host/root:ro,rslave \
  grafana/alloy:v1.17.1 run /conf/config.alloy
