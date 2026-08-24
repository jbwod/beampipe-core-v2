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
if ! beampipe_has_runtime_flag --runtime=host; then
  echo "beampipe_has_runtime_flag missed equals runtime" >&2
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

HOME_OUTPUT=$(mktemp -d)
if beampipe_require_explicit_unattended --runtime docker < /dev/null > "$HOME_OUTPUT/implicit" 2>&1; then
  echo "headless setup accepted implicit unattended mode" >&2
  exit 1
fi
if ! grep -Fq -- "--yes --runtime docker" "$HOME_OUTPUT/implicit"; then
  echo "headless setup rejection omitted the explicit command" >&2
  exit 1
fi
if ! beampipe_require_explicit_unattended --yes --runtime=host < /dev/null > /dev/null; then
  echo "explicit unattended host setup was rejected" >&2
  exit 1
fi
if beampipe_require_explicit_unattended --yes < /dev/null > "$HOME_OUTPUT/runtime" 2>&1; then
  echo "headless --yes setup accepted an implicit runtime" >&2
  exit 1
fi
if ! grep -Fq -- "--yes requires an explicit --runtime" "$HOME_OUTPUT/runtime"; then
  echo "missing-runtime rejection was unclear" >&2
  exit 1
fi
HEADLESS_BIN="$HOME_OUTPUT/bin"
mkdir -p "$HEADLESS_BIN"
printf '%s\n' '#!/bin/sh' ': > "$BEAMPIPE_CURL_CALLED"' > "$HEADLESS_BIN/curl"
chmod +x "$HEADLESS_BIN/curl"
if BEAMPIPE_CURL_CALLED="$HOME_OUTPUT/curl-called" HOME="$HOME_OUTPUT/home" \
  PATH="$HEADLESS_BIN:/usr/bin:/bin" sh "$root/deploy/install.sh" \
  < /dev/null > "$HOME_OUTPUT/headless-main" 2>&1; then
  echo "headless installer without --yes unexpectedly succeeded" >&2
  exit 1
fi
if [ -e "$HOME_OUTPUT/curl-called" ]; then
  echo "headless installer downloaded before confirming unattended intent" >&2
  exit 1
fi
beampipe_step 1 3 "Check this machine" > "$HOME_OUTPUT/step"
if ! grep -Fq "[1/3] Check this machine" "$HOME_OUTPUT/step"; then
  echo "installer progress output is unclear" >&2
  exit 1
fi
if LC_ALL=C grep -q "$(printf '\033')" "$HOME_OUTPUT/step"; then
  echo "non-terminal installer output contained ANSI escapes" >&2
  exit 1
fi
RESUME_HOME="$HOME_OUTPUT/Jack's install"
beampipe_print_setup_failure "$RESUME_HOME" 7 > "$HOME_OUTPUT/failure" 2>&1
if ! grep -Fq "Setup stopped with exit status 7" "$HOME_OUTPUT/failure"; then
  echo "setup failure omitted its exit status" >&2
  exit 1
fi
if ! grep -Fq "beampipe --home '$HOME_OUTPUT/Jack'\\''s install' setup" "$HOME_OUTPUT/failure"; then
  echo "setup failure omitted its safe resume command" >&2
  exit 1
fi
rm -rf "$HOME_OUTPUT"
echo "install progress and recovery output ok"
