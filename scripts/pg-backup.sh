#!/usr/bin/env bash
# Backup ledger, provenance, and registry tables. Requires DATABASE_URL or first arg.
set -euo pipefail
DATABASE_URL="${1:-${DATABASE_URL:-}}"
if [[ -z "${DATABASE_URL}" ]]; then
  echo "usage: DATABASE_URL=... $0" >&2
  exit 1
fi
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="${BEAMPIPE_BACKUP_DIR:-./backups}/beampipe-${STAMP}.sql"
mkdir -p "$(dirname "${OUT}")"
pg_dump "${DATABASE_URL}" \
  --no-owner \
  --table=batch_execution_record \
  --table=provenance_events \
  --table=source_registry \
  --table=archive_metadata \
  --table=jobs \
  --table=notification_channels \
  --table=alert_rules \
  --table=alert_deliveries \
  > "${OUT}"
echo "wrote ${OUT}"
