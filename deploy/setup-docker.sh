#!/bin/sh
# Prepare a Compose checkout without starting Postgres or the stack.
# Usage (from the repository root):
#   ./deploy/setup-docker.sh --yes --skip-admin --skip-upload
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

if [ ! -f docker-compose.yml ]; then
  echo "run this script from a Beampipe Core checkout that contains docker-compose.yml" >&2
  exit 1
fi

docker compose build api
exec docker compose run --rm --no-deps \
  --user "$(id -u):$(id -g)" \
  -v "$root:/checkout" -w /checkout \
  api setup --runtime docker "$@"
