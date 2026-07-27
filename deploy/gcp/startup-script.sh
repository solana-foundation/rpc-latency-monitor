#!/bin/bash
set -euo pipefail

# COS has a read-only /root; keep Docker's client config on a writable path.
export DOCKER_CONFIG=/tmp/docker
mkdir -p "$DOCKER_CONFIG"

meta() {
  curl -s -H "Metadata-Flavor: Google" \
    "http://metadata.google.internal/computeMetadata/v1/instance/attributes/$1"
}

export REGION="$(meta monitor-region)"
export IMAGE="$(meta monitor-image)"
export DOPPLER_TOKEN="$(meta doppler-token)"
export INFRA=gcp
export CONF_DIR=/var/lib/rpc-latency-monitor

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
rm -rf "$CONF_DIR"
mkdir -p "$CONF_DIR"
meta monitor-config >"$CONF_DIR/config.yaml"
meta alloy-config >"$CONF_DIR/config.alloy"

# deploy/shared/run-monitor.sh is appended below (pull + run containers).
