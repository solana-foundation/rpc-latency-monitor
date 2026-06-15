#!/bin/sh
# Usage: deploy/run-with-doppler.sh <command> [args...]
set -eu

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <command> [args...]" >&2
  exit 64
fi

if [ -z "${DOPPLER_TOKEN:-}" ]; then
  exec "$@"
fi

if ! command -v doppler >/dev/null 2>&1; then
  echo "error: DOPPLER_TOKEN is set but the doppler CLI is not installed" >&2
  exit 127
fi

preserve_env="${DOPPLER_PRESERVE_ENV:-MONITOR_REGION,RUST_LOG}"

if [ -n "${DOPPLER_PROJECT:-}" ] && [ -n "${DOPPLER_CONFIG:-}" ]; then
  exec doppler run --preserve-env="$preserve_env" \
    --project "$DOPPLER_PROJECT" --config "$DOPPLER_CONFIG" -- "$@"
fi

exec doppler run --preserve-env="$preserve_env" -- "$@"
