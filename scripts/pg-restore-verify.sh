#!/usr/bin/env bash
# Verify a Beampipe custom-format PostgreSQL backup without changing a database.
set -euo pipefail

backup_path="${1:-}"
checksum_path="${2:-${backup_path}.sha256}"

if [[ -z "${backup_path}" ]]; then
  echo "usage: $0 BACKUP.dump [BACKUP.dump.sha256]" >&2
  exit 1
fi
if [[ ! -f "${backup_path}" || ! -s "${backup_path}" ]]; then
  echo "backup is missing or empty: ${backup_path}" >&2
  exit 1
fi
if [[ ! -f "${checksum_path}" ]]; then
  echo "checksum is missing: ${checksum_path}" >&2
  exit 1
fi
if ! command -v pg_restore >/dev/null 2>&1; then
  echo "pg_restore is required" >&2
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

read -r expected_digest _ <"${checksum_path}"
if [[ ! "${expected_digest}" =~ ^[[:xdigit:]]{64}$ ]]; then
  echo "checksum file does not contain a SHA-256 digest: ${checksum_path}" >&2
  exit 1
fi
actual_digest="$(sha256_file "${backup_path}")"
if [[ "${actual_digest,,}" != "${expected_digest,,}" ]]; then
  echo "checksum mismatch for ${backup_path}" >&2
  exit 1
fi

pg_restore --list "${backup_path}" >/dev/null
echo "verified ${backup_path} (${actual_digest})"
