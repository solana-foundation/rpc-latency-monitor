#!/bin/bash
set -euo pipefail

dnf install -y docker
systemctl enable --now docker

export IMAGE="${monitor_image}"
export REGION="${monitor_region}"
export DOPPLER_TOKEN="${doppler_token}"
export INFRA=aws
export CONF_DIR=/var/lib/rpc-latency-monitor

docker rm -f rpc-monitor alloy >/dev/null 2>&1 || true

rm -rf "$CONF_DIR"
mkdir -p "$CONF_DIR"

# Configs are base64-encoded by terraform, so no heredoc sentinel or shell
# interpolation can ever truncate or mangle them.
echo "${monitor_config_b64}" | base64 -d >"$CONF_DIR/config.yaml"
echo "${alloy_config_b64}" | base64 -d >"$CONF_DIR/config.alloy"

# deploy/shared/run-monitor.sh is appended below (pull + run containers).
