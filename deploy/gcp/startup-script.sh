#!/bin/bash
set -euo pipefail

# COS has a read-only /root; keep Docker's client config on a writable path.
export DOCKER_CONFIG=/tmp/docker
mkdir -p "$DOCKER_CONFIG"

meta() {
  curl -s -H "Metadata-Flavor: Google" \
    "http://metadata.google.internal/computeMetadata/v1/instance/attributes/$1"
}

REGION="$(meta monitor-region)"
IMAGE="$(meta monitor-image)"
DOPPLER_TOKEN="$(meta doppler-token)"

# Authenticate Docker to Artifact Registry using the VM service account token.
REGISTRY_HOST="${IMAGE%%/*}"
ACCESS_TOKEN="$(curl -s -H "Metadata-Flavor: Google" \
  "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token" \
  | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')"
echo "$ACCESS_TOKEN" | docker login -u oauth2accesstoken --password-stdin "https://${REGISTRY_HOST}"

mkdir -p /etc/rpc-latency-monitor
meta monitor-config >/etc/rpc-latency-monitor/config.yaml
meta alloy-config >/etc/rpc-latency-monitor/config.alloy

ENV_FILE="/run/rpc-latency-monitor.env"
docker run --rm -e DOPPLER_TOKEN="$DOPPLER_TOKEN" dopplerhq/cli:3 \
  secrets download --no-file --format docker >"$ENV_FILE"
chmod 600 "$ENV_FILE"

docker rm -f rpc-monitor alloy >/dev/null 2>&1 || true

docker run -d --name rpc-monitor --network host --restart always \
  -e MONITOR_REGION="$REGION" -e RUST_LOG=info \
  --env-file "$ENV_FILE" \
  -v /etc/rpc-latency-monitor/config.yaml:/etc/rpc-latency-monitor/config.yaml:ro \
  "$IMAGE" --config /etc/rpc-latency-monitor/config.yaml

docker run -d --name alloy --network host --restart always \
  --env-file "$ENV_FILE" \
  -v /etc/rpc-latency-monitor/config.alloy:/etc/alloy/config.alloy:ro \
  grafana/alloy:latest run /etc/alloy/config.alloy
