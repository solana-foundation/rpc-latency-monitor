#!/bin/bash
set -euo pipefail

retry() {
  local n=0
  until "$@"; do
    n=$((n + 1))
    [ "$n" -ge 5 ] && return 1
    sleep $((n * 10))
  done
}

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

docker rm -f rpc-monitor alloy >/dev/null 2>&1 || true

REGISTRY_HOST="${IMAGE%%/*}"
docker_login() {
  local access_token
  access_token="$(curl -fsS -H "Metadata-Flavor: Google" \
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token" \
    | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')"
  [ -n "$access_token" ] || return 1
  echo "$access_token" | docker login -u oauth2accesstoken --password-stdin "https://${REGISTRY_HOST}"
}
retry docker_login

rm -rf "$CONF_DIR"
mkdir -p "$CONF_DIR"
meta monitor-config >"$CONF_DIR/config.yaml"
meta alloy-config >"$CONF_DIR/config.alloy"

