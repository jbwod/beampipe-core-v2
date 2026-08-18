#!/bin/sh
# Mapping cases for deploy/install.sh. Run: sh deploy/install-target-test.sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BEAMPIPE_INSTALL_LIB=1
# shellcheck disable=SC1091
. "$root/deploy/install.sh"

expect() {
  got=$(beampipe_release_target_from "$1" "$2")
  if [ "$got" != "$3" ]; then
    echo "expected $1 $2 -> $3, got $got" >&2
    exit 1
  fi
}

expect Linux x86_64 x86_64-unknown-linux-gnu
expect linux amd64 x86_64-unknown-linux-gnu
expect Linux aarch64 aarch64-unknown-linux-gnu
expect Darwin arm64 aarch64-apple-darwin
expect Darwin x86_64 x86_64-apple-darwin

if [ "$(beampipe_archive_name aarch64-apple-darwin)" != "beampipe-aarch64-apple-darwin.tar.gz" ]; then
  echo "archive naming is incorrect" >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
printf 'release payload\n' > "$tmp/beampipe-test.tar.gz"
if command -v sha256sum >/dev/null 2>&1; then
  checksum=$(sha256sum "$tmp/beampipe-test.tar.gz" | awk '{print $1}')
else
  checksum=$(shasum -a 256 "$tmp/beampipe-test.tar.gz" | awk '{print $1}')
fi
printf '%s  %s\n' "$checksum" beampipe-test.tar.gz > "$tmp/SHA256SUMS"
(
  cd "$tmp"
  beampipe_verify_checksum SHA256SUMS beampipe-test.tar.gz
)
printf '0%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64 > "$tmp/BADSUM"
printf '  %s\n' beampipe-test.tar.gz >> "$tmp/BADSUM"
if (
  cd "$tmp"
  beampipe_verify_checksum BADSUM beampipe-test.tar.gz >/dev/null 2>&1
); then
  echo "checksum mismatch was accepted" >&2
  exit 1
fi

if beampipe_has_flag --yes --runtime docker; then
  echo "beampipe_has_flag missed --yes" >&2
  exit 1
fi
if ! beampipe_has_flag --yes --yes --runtime docker; then
  echo "beampipe_has_flag missed present --yes" >&2
  exit 1
fi
if beampipe_has_runtime_flag --yes; then
  echo "beampipe_has_runtime_flag false positive" >&2
  exit 1
fi
if ! beampipe_has_runtime_flag --yes --runtime docker; then
  echo "beampipe_has_runtime_flag missed --runtime" >&2
  exit 1
fi
if ! beampipe_has_runtime_flag --skip-docker; then
  echo "beampipe_has_runtime_flag missed --skip-docker" >&2
  exit 1
fi
echo "install target mapping ok"

HOME_TMP=$(mktemp -d)
trap 'rm -rf "$tmp" "$HOME_TMP"' EXIT
HOME=$HOME_TMP
SHELL=/bin/bash
beampipe_persist_path "$HOME_TMP/.local/bin"
if ! grep -Fq "$HOME_TMP/.local/bin" "$HOME_TMP/.profile"; then
  echo "PATH was not added to .profile" >&2
  exit 1
fi
if ! grep -Fq "$HOME_TMP/.local/bin" "$HOME_TMP/.bashrc"; then
  echo "PATH was not added to .bashrc" >&2
  exit 1
fi
beampipe_persist_path "$HOME_TMP/.local/bin"
if [ "$(grep -c 'Added by Beampipe installer' "$HOME_TMP/.bashrc")" -ne 1 ]; then
  echo "PATH line was duplicated in .bashrc" >&2
  exit 1
fi
printf '%s\n' 'if [ -d "$HOME/.local/bin" ] ; then PATH="$HOME/.local/bin:$PATH"; fi' > "$HOME_TMP/.zshrc"
SHELL=/bin/zsh
beampipe_persist_path "$HOME_TMP/.local/bin"
if grep -Fq 'Added by Beampipe installer' "$HOME_TMP/.zshrc"; then
  echo "PATH line was added despite existing \$HOME/.local/bin" >&2
  exit 1
fi
echo "install PATH persistence ok"
