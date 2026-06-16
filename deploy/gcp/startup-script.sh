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

# Remove any containers auto-restarted from a previous boot before we touch the
# mounted config dir, so their stale bind mounts don't pin it.
docker rm -f rpc-monitor alloy >/dev/null 2>&1 || true

# Authenticate Docker to Artifact Registry using the VM service account token.
REGISTRY_HOST="${IMAGE%%/*}"
ACCESS_TOKEN="$(curl -s -H "Metadata-Flavor: Google" \
  "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token" \
  | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')"
echo "$ACCESS_TOKEN" | docker login -u oauth2accesstoken --password-stdin "https://${REGISTRY_HOST}"

# Write config into a directory and bind-mount the directory (not individual
# files) so a missing source is created as a dir, never a stray file.
CONF_DIR=/var/lib/rpc-latency-monitor
rm -rf "$CONF_DIR"
mkdir -p "$CONF_DIR"
meta monitor-config >"$CONF_DIR/config.yaml"
meta alloy-config >"$CONF_DIR/config.alloy"

ENV_FILE=/run/rpc-latency-monitor.env
docker run --rm -e DOPPLER_TOKEN="$DOPPLER_TOKEN" dopplerhq/cli:3 \
  secrets download --no-file --format docker >"$ENV_FILE"
chmod 600 "$ENV_FILE"

docker run -d --name rpc-monitor --network host --restart always \
  -e MONITOR_REGION="$REGION" -e RUST_LOG=info \
  --env-file "$ENV_FILE" \
  -v "$CONF_DIR":/conf:ro \
  "$IMAGE" --config /conf/config.yaml

docker run -d --name alloy --network host --restart always \
  --env-file "$ENV_FILE" \
  -v "$CONF_DIR":/conf:ro \
  grafana/alloy:latest run /conf/config.alloy
