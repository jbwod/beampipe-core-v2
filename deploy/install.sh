#!/bin/sh
# Install the Beampipe release binary and run setup.
# Usage:
#   curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --yes --runtime docker
# Linux archives need glibc and OpenSSL 3 (Ubuntu 22.04 / Debian bookworm or newer).
set -eu

REPO="${BEAMPIPE_REPO:-jbwod/beampipe-core-v2}"
RELEASES="https://github.com/${REPO}/releases"

beampipe_release_target_from() {
  os=$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')
  arch=$2
  case "${os}-${arch}" in
    linux-x86_64|linux-amd64) printf '%s\n' x86_64-unknown-linux-gnu ;;
    linux-aarch64|linux-arm64) printf '%s\n' aarch64-unknown-linux-gnu ;;
    darwin-arm64) printf '%s\n' aarch64-apple-darwin ;;
    darwin-x86_64) printf '%s\n' x86_64-apple-darwin ;;
    *)
      echo "unsupported platform: $1 $2 (need Linux or macOS amd64/arm64)" >&2
      return 1
      ;;
  esac
}

beampipe_release_target() {
  beampipe_release_target_from "$(uname -s)" "$(uname -m)"
}

beampipe_verify_checksum() {
  sums=$1
  archive=$2
  if command -v sha256sum >/dev/null 2>&1; then
    grep "$archive" "$sums" | sha256sum -c -
  elif command -v shasum >/dev/null 2>&1; then
    grep "$archive" "$sums" | shasum -a 256 -c
  else
    echo "need sha256sum or shasum to verify the release archive" >&2
    return 1
  fi
}

install_beampipe() {
  target=$(beampipe_release_target)
  version="${BEAMPIPE_VERSION:-latest}"
  bindir="${BEAMPIPE_BIN:-$HOME/.local/bin}"
  mkdir -p "$bindir"

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  archive="beampipe-${target}.tar.gz"

  if [ "$version" = "latest" ]; then
    base="${RELEASES}/latest/download"
  else
    case "$version" in
      v*) tag=$version ;;
      *) tag="v${version}" ;;
    esac
    base="${RELEASES}/download/${tag}"
  fi

  echo "Downloading ${archive} from ${base}"
  curl -fsSL -o "${tmp}/${archive}" "${base}/${archive}"
  curl -fsSL -o "${tmp}/SHA256SUMS" "${base}/SHA256SUMS"
  (
    cd "$tmp"
    beampipe_verify_checksum SHA256SUMS "$archive"
    tar -xzf "$archive"
  )
  install -m 0755 "${tmp}/beampipe-${target}/beampipe" "${bindir}/beampipe"
  echo "Installed ${bindir}/beampipe"

  case ":${PATH}:" in
    *":${bindir}:"*) ;;
    *)
      echo "Add ${bindir} to PATH:"
      echo "  export PATH=\"${bindir}:\$PATH\""
      PATH="${bindir}:${PATH}"
      export PATH
      ;;
  esac
}

run_setup() {
  home="${BEAMPIPE_HOME:-$HOME/beampipe}"
  exec beampipe setup --directory "$home" "$@"
}

main() {
  if ! command -v curl >/dev/null 2>&1; then
    echo "curl is required" >&2
    exit 1
  fi
  install_beampipe
  run_setup "$@"
}

if [ "${BEAMPIPE_INSTALL_LIB:-}" = "1" ]; then
  return 0 2>/dev/null || exit 0
fi

main "$@"
