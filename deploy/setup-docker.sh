#!/bin/sh
# Prepare a Compose checkout without starting Postgres or the stack.
# Usage (from the repository root):
#   ./deploy/setup-docker.sh --yes --skip-admin --skip-upload
# Pulls ghcr.io/jbwod/beampipe-core-v2:${BEAMPIPE_VERSION:-0.1.5} unless it is already local.
# Compile from this checkout instead with:
#   BEAMPIPE_BUILD=1 ./deploy/setup-docker.sh --yes --skip-admin --skip-upload
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

if [ ! -f docker-compose.yml ]; then
  echo "run this script from a Beampipe Core checkout that contains docker-compose.yml" >&2
  exit 1
fi

# Prefer an explicit pin, then .env, then .env.example, so pull matches the
# checkout before setup creates .env.
if [ -z "${BEAMPIPE_VERSION:-}" ]; then
  for f in .env .env.example; do
    if [ -f "$f" ]; then
      ver=$(grep -E '^BEAMPIPE_VERSION=' "$f" 2>/dev/null | tail -n 1 | cut -d= -f2- | tr -d '\r' || true)
      if [ -n "$ver" ]; then
        BEAMPIPE_VERSION=$ver
        break
      fi
    fi
  done
fi
if [ -n "${BEAMPIPE_VERSION:-}" ]; then
  export BEAMPIPE_VERSION
fi

if [ "${BEAMPIPE_BUILD:-0}" = "1" ]; then
  docker compose build api
elif ! docker compose pull api; then
  echo "published image unavailable; building locally (set BEAMPIPE_BUILD=1 to skip the pull)" >&2
  docker compose build api
fi

exec docker compose run --rm --no-deps \
  --user "$(id -u):$(id -g)" \
  -v "$root:/checkout" -w /checkout \
  api setup --runtime docker --no-start "$@"
