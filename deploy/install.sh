#!/bin/sh
# Install the Beampipe release binary and run setup.
# Usage:
#   curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --yes --runtime docker
#   curl -fsSL .../install.sh | sh -s -- --yes --runtime docker --postgres compose \
#     --api-port 18080 --postgres-port 5432 --metrics-port 9090
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
      ;;
  esac
  beampipe_persist_path "$bindir"
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

beampipe_path_export() {
  printf 'export PATH="%s:$PATH"\n' "$1"
}

beampipe_rc_mentions_bindir() {
  file=$1
  bindir=$2
  [ -f "$file" ] || return 1
  if grep -Fq "$bindir" "$file"; then
    return 0
  fi
  case "$bindir" in
    "$HOME/.local/bin"|*/.local/bin)
      if grep -Eq '(^|[^[:alnum:]_])(\$HOME|~)/\.local/bin' "$file"; then
        return 0
      fi
      ;;
  esac
  return 1
}

beampipe_append_path_rc() {
  file=$1
  bindir=$2
  if beampipe_rc_mentions_bindir "$file" "$bindir"; then
    return 0
  fi
  mkdir -p "$(dirname "$file")"
  if [ -f "$file" ] && [ -s "$file" ]; then
    printf '\n' >> "$file"
  fi
  {
    echo "# Added by Beampipe installer"
    beampipe_path_export "$bindir"
  } >> "$file"
  echo "Added ${bindir} to PATH in ${file}"
}

beampipe_persist_path() {
  bindir=$1
  beampipe_append_path_rc "${HOME}/.profile" "$bindir"
  shellname=$(basename "${SHELL:-}")
  case "$shellname" in
    zsh)
      beampipe_append_path_rc "${HOME}/.zprofile" "$bindir"
      beampipe_append_path_rc "${HOME}/.zshrc" "$bindir"
      ;;
    fish)
      fish_file="${HOME}/.config/fish/config.fish"
      if [ -f "$fish_file" ] && grep -Fq "$bindir" "$fish_file"; then
        return 0
      fi
      mkdir -p "$(dirname "$fish_file")"
      printf 'fish_add_path %s\n' "$bindir" >> "$fish_file"
      echo "Added ${bindir} to PATH in ${fish_file}"
      ;;
    *)
      beampipe_append_path_rc "${HOME}/.bashrc" "$bindir"
      if [ -f "${HOME}/.bash_profile" ]; then
        beampipe_append_path_rc "${HOME}/.bash_profile" "$bindir"
      fi
      ;;
  esac
}

beampipe_print_path_hint() {
  bindir="${BEAMPIPE_BIN:-$HOME/.local/bin}"
  echo
  echo "The beampipe command is ${bindir}/beampipe"
  echo "This installer already updated PATH for its own process. This terminal may still need:"
  echo "  export PATH=\"${bindir}:\$PATH\""
}

beampipe_run_cli_setup() {
  home=$1
  shift
  beampipe --home "$home" setup "$@"
}

run_setup() {
  home="${BEAMPIPE_HOME:-$HOME/beampipe}"
  status=0
  if [ -t 0 ] || beampipe_has_flag --yes "$@"; then
    beampipe_run_cli_setup "$home" "$@" || status=$?
  elif [ -t 1 ] && [ -c /dev/tty ]; then
    echo "stdin is a pipe; reading setup prompts from the terminal."
    beampipe_run_cli_setup "$home" "$@" </dev/tty || status=$?
  elif ! beampipe_has_runtime_flag "$@"; then
    echo "stdin is not a terminal; running non-interactive Docker setup."
    echo "  curl -fsSL ${RELEASES}/latest/download/install.sh | sh -s -- --yes --runtime docker"
    beampipe_run_cli_setup "$home" --yes --runtime docker "$@" || status=$?
  else
    echo "stdin is not a terminal; adding --yes."
    beampipe_run_cli_setup "$home" --yes "$@" || status=$?
  fi
  beampipe_print_path_hint
  return "$status"
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
