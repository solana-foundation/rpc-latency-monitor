docker pull "$IMAGE"

ENV_FILE=/run/rpc-latency-monitor.env
docker run --rm -e DOPPLER_TOKEN="$DOPPLER_TOKEN" dopplerhq/cli:3 \
  secrets download --no-file --format docker >"$ENV_FILE"
chmod 600 "$ENV_FILE"

docker run -d --name rpc-monitor --network host --restart always \
  -e MONITOR_REGION="$REGION" -e MONITOR_INFRA="$INFRA" -e RUST_LOG=info \
  --env-file "$ENV_FILE" \
  -v "$CONF_DIR":/conf:ro \
  "$IMAGE" --config /conf/config.yaml

docker run -d --name alloy --network host --restart always \
  -e MONITOR_INFRA="$INFRA" \
  --env-file "$ENV_FILE" \
  -v "$CONF_DIR":/conf:ro \
  grafana/alloy:v1.17.1 run /conf/config.alloy
