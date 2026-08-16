#!/bin/sh
# Validate Prometheus alert rules. Used by Rust CI.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
rules="$root/deploy/prometheus/alerts.yml"

if command -v promtool >/dev/null 2>&1; then
  promtool check rules "$rules"
  exit 0
fi

if command -v docker >/dev/null 2>&1; then
  docker run --rm --entrypoint promtool \
    -v "$root/deploy/prometheus:/rules:ro" \
    prom/prometheus:v2.55.1 \
    check rules /rules/alerts.yml
  exit 0
fi

echo "need promtool or docker to check $rules" >&2
exit 1
