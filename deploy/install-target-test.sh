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
