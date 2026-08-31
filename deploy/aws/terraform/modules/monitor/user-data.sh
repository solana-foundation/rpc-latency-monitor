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

mkdir -p /etc/systemd/resolved.conf.d
cat >/etc/systemd/resolved.conf.d/99-rpc-monitor-dns.conf <<'EOF'
[Resolve]
DNS=8.8.8.8 8.8.4.4
Domains=~.
EOF
systemctl restart systemd-resolved

retry dnf install -y docker
systemctl enable --now docker

export IMAGE="${monitor_image}"
export REGION="${monitor_region}"
export DOPPLER_TOKEN="${doppler_token}"
export INFRA=aws
export CONF_DIR=/var/lib/rpc-latency-monitor

docker rm -f rpc-monitor alloy >/dev/null 2>&1 || true

rm -rf "$CONF_DIR"
mkdir -p "$CONF_DIR"

echo "${monitor_config_b64}" | base64 -d >"$CONF_DIR/config.yaml"
echo "${alloy_config_b64}" | base64 -d >"$CONF_DIR/config.alloy"

