#!/bin/sh
# Install the Beampipe release binary and run setup.
# Usage:
#   curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --yes --runtime docker
#   curl -fsSL .../install.sh | sh -s -- --yes --runtime docker --postgres compose \
#     --api-port 18080 --postgres-port 5432 --metrics-port 9090
#   curl -fsSL .../install.sh | sh -s -- --yes --runtime docker --use-real-backends
# Linux archives need glibc and OpenSSL 3 (Ubuntu 22.04 / Debian bookworm or newer).
# After the binary is installed this runs `beampipe setup`. The CLI owns the
# guided questions, summary, and next-action recipe. Headless callers must pass
# --yes and an explicit runtime; the wrapper never guesses unattended intent.
set -eu

REPO="${BEAMPIPE_REPO:-jbwod/beampipe-core-v2}"
RELEASES="https://github.com/${REPO}/releases"
BEAMPIPE_UI_BOLD=
BEAMPIPE_UI_DIM=
BEAMPIPE_UI_CYAN=
BEAMPIPE_UI_GREEN=
BEAMPIPE_UI_RED=
BEAMPIPE_UI_RESET=

beampipe_init_ui() {
  if [ -t 1 ] && [ "${NO_COLOR+x}" != "x" ] && [ "${TERM:-}" != "dumb" ]; then
    BEAMPIPE_UI_BOLD=$(printf '\033[1m')
    BEAMPIPE_UI_DIM=$(printf '\033[2m')
    BEAMPIPE_UI_CYAN=$(printf '\033[36m')
    BEAMPIPE_UI_GREEN=$(printf '\033[32m')
    BEAMPIPE_UI_RED=$(printf '\033[31m')
    BEAMPIPE_UI_RESET=$(printf '\033[0m')
  fi
}

beampipe_step() {
  current=$1
  total=$2
  label=$3
  printf '\n%s[%s/%s]%s %s%s%s\n' \
    "$BEAMPIPE_UI_CYAN" "$current" "$total" "$BEAMPIPE_UI_RESET" \
    "$BEAMPIPE_UI_BOLD" "$label" "$BEAMPIPE_UI_RESET"
}

beampipe_detail() {
  label=$1
  value=$2
  printf '      %s%-12s%s %s\n' "$BEAMPIPE_UI_DIM" "${label}:" "$BEAMPIPE_UI_RESET" "$value"
}

beampipe_ok() {
  printf '      %s[ok]%s %s\n' "$BEAMPIPE_UI_GREEN" "$BEAMPIPE_UI_RESET" "$1"
}

beampipe_error() {
  printf '      %s[error]%s %s\n' "$BEAMPIPE_UI_RED" "$BEAMPIPE_UI_RESET" "$1" >&2
}

beampipe_require_command() {
  if command -v "$1" >/dev/null 2>&1; then
    return 0
  fi
  beampipe_error "${1} is required but was not found on PATH"
  return 1
}

beampipe_check_install_requirements() {
  for required in awk basename curl dirname grep install mkdir mktemp rm sed tar tr uname; do
    beampipe_require_command "$required" || return 1
  done
  if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    beampipe_error "sha256sum or shasum is required to verify the release"
    return 1
  fi
}

beampipe_shell_quote() {
  printf "'"
  printf '%s' "$1" | sed "s/'/'\\\\''/g"
  printf "'"
}

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
  beampipe_ok "SHA-256 verified for ${archive}"
}

beampipe_archive_name() {
  printf 'beampipe-%s.tar.gz\n' "$1"
}

install_beampipe() {
  target=$(beampipe_release_target)
  version="${BEAMPIPE_VERSION:-latest}"
  bindir="${BEAMPIPE_BIN:-$HOME/.local/bin}"
  mkdir -p "$bindir"

  beampipe_step 2 3 "Download and install"
  beampipe_detail "Release" "$version"
  beampipe_detail "Platform" "$target"
  beampipe_detail "Binary" "${bindir}/beampipe"

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

  beampipe_detail "Download" "${base}/${archive}"
  curl -fsSL -o "${tmp}/${archive}" "${base}/${archive}"
  curl -fsSL -o "${tmp}/SHA256SUMS" "${base}/SHA256SUMS"
  (
    cd "$tmp"
    beampipe_verify_checksum SHA256SUMS "$archive"
    tar -xzf "$archive"
  )
  install -m 0755 "${tmp}/beampipe-${target}/beampipe" "${bindir}/beampipe"
  beampipe_ok "Installed ${bindir}/beampipe"

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
    case "$arg" in
      --runtime=*) return 0 ;;
    esac
    prev=$arg
  done
  return 1
}

beampipe_require_explicit_unattended() {
  if [ -t 0 ] || { [ -t 1 ] && [ -c /dev/tty ]; }; then
    return 0
  fi
  if ! beampipe_has_flag --yes "$@"; then
    beampipe_error "No interactive terminal is available and --yes was not supplied"
    echo "Run unattended setup explicitly:" >&2
    echo "  curl -fsSL ${RELEASES}/latest/download/install.sh | sh -s -- --yes --runtime docker" >&2
    return 2
  fi
  if ! beampipe_has_runtime_flag "$@"; then
    beampipe_error "--yes requires an explicit --runtime docker or --runtime host"
    return 2
  fi
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
  echo "Command installed: ${bindir}/beampipe"
  echo "A new terminal will pick up the PATH change. For this terminal, run:"
  echo "  export PATH=\"${bindir}:\$PATH\""
}

beampipe_print_setup_failure() {
  home=$1
  status=$2
  quoted_home=$(beampipe_shell_quote "$home")
  echo
  beampipe_error "Setup stopped with exit status ${status}"
  echo "The verified beampipe binary is installed; no automatic rollback was attempted."
  echo "Review the message above, then resume safely with:"
  echo "  beampipe --home ${quoted_home} setup"
}

beampipe_run_cli_setup() {
  home=$1
  shift
  beampipe --home "$home" setup "$@"
}

run_setup() {
  home="${BEAMPIPE_HOME:-$HOME/beampipe}"
  status=0
  beampipe_step 3 3 "Configure Beampipe"
  beampipe_detail "Home" "$home"
  if [ -t 0 ] || beampipe_has_flag --yes "$@"; then
    if beampipe_has_flag --yes "$@"; then
      beampipe_detail "Mode" "unattended"
    else
      beampipe_detail "Mode" "guided wizard"
    fi
    beampipe_run_cli_setup "$home" "$@" || status=$?
  elif [ -t 1 ] && [ -c /dev/tty ]; then
    beampipe_detail "Mode" "guided wizard (prompts use this terminal)"
    beampipe_run_cli_setup "$home" "$@" </dev/tty || status=$?
  else
    beampipe_error "No interactive terminal is available and --yes was not supplied"
    return 2
  fi
  beampipe_print_path_hint
  if [ "$status" -ne 0 ]; then
    beampipe_print_setup_failure "$home" "$status"
  fi
  return "$status"
}

main() {
  beampipe_init_ui
  beampipe_step 1 3 "Check this machine"
  beampipe_check_install_requirements
  beampipe_require_explicit_unattended "$@"
  beampipe_detail "System" "$(uname -s) $(uname -m)"
  beampipe_ok "Installer requirements available"
  install_beampipe
  run_setup "$@"
}

if [ "${BEAMPIPE_INSTALL_LIB:-}" = "1" ]; then
  return 0 2>/dev/null || exit 0
fi

main "$@"
