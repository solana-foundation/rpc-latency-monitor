#!/bin/bash
set -euo pipefail

export DOCKER_CONFIG=/tmp/docker
mkdir -p "$DOCKER_CONFIG"

meta() {
  curl -s -H "Metadata-Flavor: Google" \
    "http://metadata.google.internal/computeMetadata/v1/instance/attributes/$1"
}

sysctl -w net.ipv4.tcp_slow_start_after_idle=0 \
  net.ipv4.tcp_fastopen=3 \
  net.ipv4.tcp_mtu_probing=1 \
  net.core.rmem_max=33554432 \
  net.core.wmem_max=33554432 \
  net.core.default_qdisc=fq >/dev/null 2>&1 || true
sysctl -w net.ipv4.tcp_rmem="4096 262144 33554432" net.ipv4.tcp_wmem="4096 262144 33554432" >/dev/null 2>&1 || true
modprobe tcp_bbr >/dev/null 2>&1 && sysctl -w net.ipv4.tcp_congestion_control=bbr >/dev/null 2>&1 || true

export REGION="$(meta monitor-region)"
export IMAGE="$(meta monitor-image)"
export DOPPLER_TOKEN="$(meta doppler-token)"
export INFRA=gcp
export CONF_DIR=/var/lib/rpc-latency-monitor

docker rm -f rpc-monitor alloy >/dev/null 2>&1 || true

REGISTRY_HOST="${IMAGE%%/*}"
ACCESS_TOKEN="$(curl -s -H "Metadata-Flavor: Google" \
  "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token" \
  | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')"
echo "$ACCESS_TOKEN" | docker login -u oauth2accesstoken --password-stdin "https://${REGISTRY_HOST}"

rm -rf "$CONF_DIR"
mkdir -p "$CONF_DIR"
meta monitor-config >"$CONF_DIR/config.yaml"
meta alloy-config >"$CONF_DIR/config.alloy"

