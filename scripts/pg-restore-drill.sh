#!/usr/bin/env bash
# Restore a verified backup into an explicitly named disposable drill database.
set -euo pipefail

backup_path="${1:-}"
target_url="${2:-${BEAMPIPE_RESTORE_DRILL_URL:-}}"

if [[ -z "${backup_path}" || -z "${target_url}" ]]; then
  echo "usage: BEAMPIPE_RESTORE_DRILL_URL=postgresql://.../beampipe_restore_drill_NAME $0 BACKUP.dump" >&2
  exit 1
fi
if ! command -v psql >/dev/null 2>&1 || ! command -v pg_restore >/dev/null 2>&1; then
  echo "psql and pg_restore are required" >&2
  exit 1
fi

"$(dirname "$0")/pg-restore-verify.sh" "${backup_path}"

database_name="$(psql "${target_url}" --no-psqlrc --tuples-only --no-align --command 'SELECT current_database()')"
if [[ ! "${database_name}" =~ ^beampipe_restore_drill_[A-Za-z0-9_]+$ ]]; then
  echo "refusing restore: target database must be named beampipe_restore_drill_NAME (got ${database_name})" >&2
  exit 1
fi

table_count="$(psql "${target_url}" --no-psqlrc --tuples-only --no-align --set ON_ERROR_STOP=1 --command \
  "SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname NOT IN ('pg_catalog', 'information_schema')")"
if [[ "${table_count}" != "0" ]]; then
  echo "refusing restore: drill database ${database_name} is not empty" >&2
  exit 1
fi

pg_restore \
  --dbname="${target_url}" \
  --exit-on-error \
  --single-transaction \
  --no-owner \
  --no-privileges \
  "${backup_path}"

missing_tables="$(psql "${target_url}" --no-psqlrc --tuples-only --no-align --set ON_ERROR_STOP=1 --command \
  "SELECT string_agg(name, ', ' ORDER BY name)
     FROM unnest(ARRAY['batch_execution_record','jobs','source_registry','project_configs']) AS name
    WHERE to_regclass('public.' || name) IS NULL")"
if [[ -n "${missing_tables}" ]]; then
  echo "restore drill failed: missing core tables: ${missing_tables}" >&2
  exit 1
fi

psql "${target_url}" --no-psqlrc --set ON_ERROR_STOP=1 --command \
  "SELECT 'batch_execution_record' AS table_name, count(*) AS rows FROM batch_execution_record
   UNION ALL SELECT 'jobs', count(*) FROM jobs
   UNION ALL SELECT 'source_registry', count(*) FROM source_registry
   UNION ALL SELECT 'project_configs', count(*) FROM project_configs"
echo "restore drill passed in ${database_name}; inspect it, then drop that disposable database explicitly"
