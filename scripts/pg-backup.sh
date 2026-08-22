#!/usr/bin/env bash
# Create a complete, verified PostgreSQL backup. Requires DATABASE_URL or first arg.
set -euo pipefail

umask 077

database_url="${1:-${DATABASE_URL:-}}"
backup_dir="${BEAMPIPE_BACKUP_DIR:-./backups}"
retention_days="${BEAMPIPE_BACKUP_RETENTION_DAYS:-30}"

if [[ -z "${database_url}" ]]; then
  echo "usage: DATABASE_URL=... $0 [DATABASE_URL]" >&2
  exit 1
fi
if [[ ! "${retention_days}" =~ ^[1-9][0-9]*$ ]]; then
  echo "BEAMPIPE_BACKUP_RETENTION_DAYS must be a positive integer" >&2
  exit 1
fi
if ! command -v pg_dump >/dev/null 2>&1 || ! command -v pg_restore >/dev/null 2>&1; then
  echo "pg_dump and pg_restore are required" >&2
  exit 1
fi

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    echo "sha256sum or shasum is required" >&2
    return 1
  fi
}

mkdir -p -- "${backup_dir}"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
backup_path="${backup_dir}/beampipe-${stamp}.dump"
checksum_path="${backup_path}.sha256"
if [[ -e "${backup_path}" || -e "${checksum_path}" ]]; then
  echo "refusing to overwrite existing backup ${backup_path}" >&2
  exit 1
fi

temporary_backup="$(mktemp "${backup_dir}/.beampipe-${stamp}.XXXXXX.dump")"
temporary_checksum="$(mktemp "${backup_dir}/.beampipe-${stamp}.XXXXXX.sha256")"
cleanup() {
  rm -f -- "${temporary_backup}" "${temporary_checksum}"
}
trap cleanup EXIT

pg_dump "${database_url}" \
  --format=custom \
  --compress=6 \
  --no-owner \
  --no-privileges \
  --file="${temporary_backup}"

# Do not publish a dump that PostgreSQL cannot parse.
pg_restore --list "${temporary_backup}" >/dev/null
digest="$(sha256_file "${temporary_backup}")"
printf '%s  %s\n' "${digest}" "$(basename "${backup_path}")" >"${temporary_checksum}"

# Both files are built in the destination filesystem, so each publication is atomic.
mv -- "${temporary_backup}" "${backup_path}"
mv -- "${temporary_checksum}" "${checksum_path}"

while IFS= read -r -d '' expired_backup; do
  rm -f -- "${expired_backup}" "${expired_backup}.sha256"
done < <(find "${backup_dir}" -maxdepth 1 -type f -name 'beampipe-*.dump' -mtime "+${retention_days}" -print0)

echo "wrote ${backup_path}"
echo "checksum ${checksum_path}"
