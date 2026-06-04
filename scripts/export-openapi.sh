#!/usr/bin/env bash
# Write OpenAPI spec to openapi.json (repo root) for ReDoc / Read the Docs.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT="${1:-$ROOT/openapi.json}"
cargo run -q -p beampipe-cli -- openapi export >"$OUT"
paths="$(python3 -c "import json; print(len(json.load(open('$OUT'))['paths']))")"
echo "Wrote $OUT ($paths paths)"
