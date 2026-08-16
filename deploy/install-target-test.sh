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
echo "install target mapping ok"
