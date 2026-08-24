#!/bin/sh
# Validate Prometheus alert rules. Used by Rust CI.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
rules="$root/deploy/prometheus/alerts.yml"
alertmanager="$root/deploy/alertmanager/alertmanager.yml"

python3 -m json.tool "$root/deploy/grafana/dashboards/beampipe-overview.json" >/dev/null
if grep -R '\${' \
  "$root/deploy/prometheus.yml" \
  "$root/deploy/prometheus/alerts.yml" \
  "$root/deploy/alertmanager/alertmanager.yml" \
  "$root/deploy/grafana" >/dev/null; then
  echo "observability config contains an unexpanded shell placeholder" >&2
  exit 1
fi

if command -v promtool >/dev/null 2>&1 && command -v amtool >/dev/null 2>&1; then
  promtool check rules "$rules"
  amtool check-config "$alertmanager"
  exit 0
fi

if command -v docker >/dev/null 2>&1; then
  docker run --rm --entrypoint promtool \
    -v "$root/deploy/prometheus:/rules:ro" \
    prom/prometheus:v3.5.5 \
    check rules /rules/alerts.yml
  docker run --rm --entrypoint amtool \
    -v "$root/deploy/alertmanager:/config:ro" \
    prom/alertmanager:v0.33.1 \
    check-config /config/alertmanager.yml
  exit 0
fi

echo "need promtool plus amtool, or docker, to check observability config" >&2
exit 1
