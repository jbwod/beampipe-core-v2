#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
output_path=${1:-"$repo_root/boilerplate_docs/assets/data/database-schema.json"}
postgres_image=${BEAMPIPE_SCHEMA_POSTGRES_IMAGE:-postgres:16-alpine}
docker_context=${BEAMPIPE_DOCKER_CONTEXT:-}
container_name="beampipe-schema-docs-$$-$RANDOM"
temporary_output=""

docker_command=(docker)
if [[ -n "$docker_context" ]]; then
    docker_command+=(--context "$docker_context")
fi

cleanup() {
    "${docker_command[@]}" rm -f "$container_name" >/dev/null 2>&1 || true
    if [[ -n "$temporary_output" && -f "$temporary_output" ]]; then
        rm -f "$temporary_output"
    fi
}
trap cleanup EXIT

mapfile -t migrations < <(find "$repo_root/migrations" -maxdepth 1 -type f -name '*.sql' | sort)
if [[ ${#migrations[@]} -eq 0 ]]; then
    echo "no SQL migrations found under $repo_root/migrations" >&2
    exit 1
fi

latest_migration=$(basename "${migrations[${#migrations[@]} - 1]}" .sql)

"${docker_command[@]}" run --rm -d \
    --name "$container_name" \
    --mount "type=bind,src=$repo_root/migrations,dst=/migrations,readonly" \
    -e POSTGRES_HOST_AUTH_METHOD=trust \
    "$postgres_image" >/dev/null

for _ in {1..30}; do
    if "${docker_command[@]}" exec "$container_name" \
        pg_isready -U postgres >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

if ! "${docker_command[@]}" exec "$container_name" \
    pg_isready -U postgres >/dev/null 2>&1; then
    echo "disposable PostgreSQL did not become ready" >&2
    exit 1
fi

for migration in "${migrations[@]}"; do
    "${docker_command[@]}" exec -i "$container_name" \
        psql -v ON_ERROR_STOP=1 -U postgres -d postgres \
        < "$migration" >/dev/null
done

mkdir -p "$(dirname "$output_path")"
temporary_output=$(mktemp "${output_path}.tmp.XXXXXX")

"${docker_command[@]}" exec -i "$container_name" \
    psql -v ON_ERROR_STOP=1 -A -t -U postgres -d postgres \
    -v "migration_count=${#migrations[@]}" \
    -v "latest_migration=$latest_migration" \
    < "$repo_root/scripts/database-schema.sql" \
    > "$temporary_output"

python3 -m json.tool "$temporary_output" >/dev/null
mv "$temporary_output" "$output_path"
temporary_output=""

echo "wrote $output_path from ${#migrations[@]} migrations"
