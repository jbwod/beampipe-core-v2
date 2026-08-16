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
  expected=$(awk -v archive="$archive" '$2 == archive || $2 == "*" archive { print $1; found = 1; exit } END { if (!found) exit 1 }' "$sums") || {
    echo "checksum entry missing for ${archive}" >&2
    return 1
  }
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$archive" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$archive" | awk '{print $1}')
  else
    echo "need sha256sum or shasum to verify the release archive" >&2
    return 1
  fi
  if [ "$actual" != "$expected" ]; then
    echo "checksum verification failed for ${archive}" >&2
    return 1
  fi
  echo "${archive}: OK"
}

beampipe_archive_name() {
  printf 'beampipe-%s.tar.gz\n' "$1"
}

install_beampipe() {
  target=$(beampipe_release_target)
  version="${BEAMPIPE_VERSION:-latest}"
  bindir="${BEAMPIPE_BIN:-$HOME/.local/bin}"
  mkdir -p "$bindir"

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  archive=$(beampipe_archive_name "$target")

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
      PATH="${bindir}:${PATH}"
      export PATH
      echo "This session can run beampipe. For new terminals:"
      echo "  echo 'export PATH=\"${bindir}:\$PATH\"' >> ~/.profile && export PATH=\"${bindir}:\$PATH\""
      beampipe_offer_persist_path "$bindir"
      ;;
  esac
}

beampipe_has_flag() {
  want=$1
  shift
  for arg in "$@"; do
    [ "$arg" = "$want" ] && return 0
  done
  return 1
}

beampipe_has_runtime_flag() {
  prev=
  for arg in "$@"; do
    if [ "$prev" = "--runtime" ] || [ "$arg" = "--docker" ] || [ "$arg" = "--skip-docker" ]; then
      return 0
    fi
    prev=$arg
  done
  return 1
}

beampipe_offer_persist_path() {
  bindir=$1
  if [ ! -t 1 ] || [ ! -c /dev/tty ]; then
    return 0
  fi
  printf "Append that line to ~/.profile now? [y/N] " > /dev/tty
  ans=
  IFS= read -r ans < /dev/tty || return 0
  case $ans in
    y|Y|yes|YES)
      if [ -f "$HOME/.profile" ] && grep -F "export PATH=\"${bindir}:\$PATH\"" "$HOME/.profile" >/dev/null 2>&1; then
        echo "PATH line already in ~/.profile"
        return 0
      fi
      echo "export PATH=\"${bindir}:\$PATH\"" >> "$HOME/.profile"
      echo "Wrote ~/.profile"
      ;;
  esac
}

run_setup() {
  home="${BEAMPIPE_HOME:-$HOME/beampipe}"
  if [ -t 0 ] || beampipe_has_flag --yes "$@"; then
    exec beampipe --home "$home" setup "$@"
  fi
  if [ -t 1 ] && [ -c /dev/tty ]; then
    echo "stdin is a pipe; reading setup prompts from the terminal."
    exec beampipe --home "$home" setup "$@" </dev/tty
  fi
  if ! beampipe_has_runtime_flag "$@"; then
    echo "stdin is not a terminal; running non-interactive Docker setup."
    echo "  curl -fsSL ${RELEASES}/latest/download/install.sh | sh -s -- --yes --runtime docker"
    exec beampipe --home "$home" setup --yes --runtime docker "$@"
  fi
  echo "stdin is not a terminal; adding --yes."
  exec beampipe --home "$home" setup --yes "$@"
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
