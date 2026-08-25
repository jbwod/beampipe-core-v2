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

expect_equal() {
  if [ "$1" != "$2" ]; then
    echo "expected [$2], got [$1]" >&2
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
unset BEAMPIPE_HOME || true

expect_equal "$(beampipe_effective_home)" "$HOME_TMP/beampipe"
expect_equal "$(BEAMPIPE_HOME="$HOME_TMP/environment home" beampipe_effective_home)" \
  "$HOME_TMP/environment home"
expect_equal "$(beampipe_effective_home --home "$HOME_TMP/global home")" \
  "$HOME_TMP/global home"
expect_equal "$(beampipe_effective_home --home="$HOME_TMP/equals home")" \
  "$HOME_TMP/equals home"
expect_equal "$(beampipe_effective_home --directory "$HOME_TMP/setup directory" \
  --home "$HOME_TMP/ignored home")" "$HOME_TMP/setup directory"
expect_equal "$(beampipe_effective_home --home "$HOME_TMP/ignored home" \
  --directory="$HOME_TMP/setup directory")" "$HOME_TMP/setup directory"
expect_equal "$(beampipe_effective_home --directory '~/tilde home')" \
  "$HOME_TMP/tilde home"
(
  cd "$HOME_TMP"
  expect_equal "$(beampipe_effective_home --directory 'relative home')" \
    "$(pwd -P)/relative home"
  expect_equal "$(beampipe_effective_home --home '../sibling home')" \
    "$(pwd -P)/../sibling home"
)
if beampipe_effective_home --home > /dev/null 2>&1; then
  echo "missing --home value was accepted" >&2
  exit 1
fi
if beampipe_effective_home --directory= > /dev/null 2>&1; then
  echo "empty --directory value was accepted" >&2
  exit 1
fi
BAD_HOME=$(printf 'bad\ninstallation')
if beampipe_effective_home --home "$BAD_HOME" > /dev/null 2>&1; then
  echo "multiline installation home was accepted" >&2
  exit 1
fi
echo "install home selection ok"

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
SAFETY_TMP=$(mktemp -d)
trap 'rm -rf "$tmp" "$HOME_TMP" "$HOME_OUTPUT" "$SAFETY_TMP"' EXIT
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

PATH_SUB_MARKER="$SAFETY_TMP/path-substitution-ran"
PATH_TICK_MARKER="$SAFETY_TMP/path-backtick-ran"
PATH_SEPARATOR_MARKER="$SAFETY_TMP/path-separator-ran"
SPECIAL_HOME=$(printf "%s/home space ' \$HOME \$(touch %s) \`touch %s\` \\tail" \
  "$SAFETY_TMP" "$PATH_SUB_MARKER" "$PATH_TICK_MARKER")
SPECIAL_ARG=$(printf "%s/project space ' \$USER; touch %s; \$(touch %s) \`touch %s\` \\tail" \
  "$SAFETY_TMP" "$PATH_SEPARATOR_MARKER" "$PATH_SUB_MARKER" "$PATH_TICK_MARKER")
beampipe_print_setup_failure "$SPECIAL_HOME" 9 \
  --home "$SAFETY_TMP/ignored global home" \
  --yes --runtime docker \
  --directory "$SPECIAL_HOME" \
  --project-config "$SPECIAL_ARG" > "$SAFETY_TMP/recovery" 2>&1
RECOVERY_COMMAND=$(awk '/^  beampipe / { sub(/^  /, ""); print; exit }' "$SAFETY_TMP/recovery")
if [ -z "$RECOVERY_COMMAND" ]; then
  echo "setup failure omitted its recovery command" >&2
  exit 1
fi
if printf '%s\n' "$RECOVERY_COMMAND" | grep -Fq -- "--directory"; then
  echo "setup failure repeated the compatibility home option" >&2
  exit 1
fi

MOCK_BIN="$SAFETY_TMP/mock-bin"
mkdir -p "$MOCK_BIN"
printf '%s\n' \
  '#!/bin/sh' \
  'printf '\''%s\n'\'' "$@" > "$BEAMPIPE_TEST_ARGS_FILE"' \
  'printf '\''%s\n'\'' "${BEAMPIPE_HOME-}" > "$BEAMPIPE_TEST_HOME_FILE"' \
  > "$MOCK_BIN/beampipe"
chmod +x "$MOCK_BIN/beampipe"
(
  cd "$SAFETY_TMP"
  PATH="$MOCK_BIN:/usr/bin:/bin"
  export PATH
  BEAMPIPE_TEST_ARGS_FILE="$SAFETY_TMP/recovery-args"
  BEAMPIPE_TEST_HOME_FILE="$SAFETY_TMP/recovery-home-env"
  export BEAMPIPE_TEST_ARGS_FILE BEAMPIPE_TEST_HOME_FILE
  eval "$RECOVERY_COMMAND"
)
printf '%s\n' --home "$SPECIAL_HOME" setup --yes --runtime docker \
  --project-config "$SPECIAL_ARG" > "$SAFETY_TMP/expected-recovery-args"
if ! cmp -s "$SAFETY_TMP/expected-recovery-args" "$SAFETY_TMP/recovery-args"; then
  echo "setup failure did not preserve safely quoted arguments" >&2
  exit 1
fi
for marker in "$PATH_SUB_MARKER" "$PATH_TICK_MARKER" "$PATH_SEPARATOR_MARKER"; do
  if [ -e "$marker" ]; then
    echo "setup recovery command executed path content" >&2
    exit 1
  fi
done

BEAMPIPE_TEST_ARGS_FILE="$SAFETY_TMP/setup-args"
BEAMPIPE_TEST_HOME_FILE="$SAFETY_TMP/setup-home-env"
export BEAMPIPE_TEST_ARGS_FILE BEAMPIPE_TEST_HOME_FILE
PATH="$MOCK_BIN:/usr/bin:/bin"
export PATH
beampipe_run_cli_setup "$SPECIAL_HOME" --yes --runtime docker --directory "$SPECIAL_HOME"
expect_equal "$(sed -n '1p' "$SAFETY_TMP/setup-home-env")" "$SPECIAL_HOME"
printf '%s\n' --home "$SPECIAL_HOME" setup --yes --runtime docker \
  > "$SAFETY_TMP/expected-setup-args"
if ! cmp -s "$SAFETY_TMP/expected-setup-args" "$SAFETY_TMP/setup-args"; then
  echo "CLI setup handoff changed caller arguments" >&2
  exit 1
fi
(
  cd "$SAFETY_TMP"
  RELATIVE_HOME=$(beampipe_effective_home --home "relative core")
  BEAMPIPE_TEST_ARGS_FILE="$SAFETY_TMP/relative-setup-args"
  BEAMPIPE_TEST_HOME_FILE="$SAFETY_TMP/relative-setup-home-env"
  export BEAMPIPE_TEST_ARGS_FILE BEAMPIPE_TEST_HOME_FILE
  beampipe_run_cli_setup "$RELATIVE_HOME" --yes --runtime docker --home "relative core"
)
printf '%s\n' --home "$SAFETY_TMP/relative core" setup --yes --runtime docker \
  > "$SAFETY_TMP/expected-relative-setup-args"
if ! cmp -s "$SAFETY_TMP/expected-relative-setup-args" "$SAFETY_TMP/relative-setup-args"; then
  echo "relative setup home was not normalized before the CLI handoff" >&2
  exit 1
fi

SPECIAL_BIN=$(printf "%s/bin space ' \$HOME \$(touch %s) \`touch %s\` \\tail" \
  "$SAFETY_TMP" "$PATH_SUB_MARKER" "$PATH_TICK_MARKER")
PATH_LINE=$(beampipe_path_export "$SPECIAL_BIN")
(
  cd "$SAFETY_TMP"
  PATH=/usr/bin:/bin
  eval "$PATH_LINE"
  expect_equal "${PATH%%:*}" "$SPECIAL_BIN"
)
BEAMPIPE_BIN="$SPECIAL_BIN" beampipe_print_path_hint > "$SAFETY_TMP/path-hint"
HINT_LINE=$(awk '/^  export PATH=/ { sub(/^  /, ""); print; exit }' "$SAFETY_TMP/path-hint")
(
  cd "$SAFETY_TMP"
  PATH=/usr/bin:/bin
  eval "$HINT_LINE"
  expect_equal "${PATH%%:*}" "$SPECIAL_BIN"
)

SAFE_HOME="$SAFETY_TMP/path-home"
mkdir -p "$SAFE_HOME"
HOME=$SAFE_HOME
SHELL=/bin/bash
beampipe_persist_path "$SPECIAL_BIN" > "$SAFETY_TMP/path-persist-output"
(
  PATH=/usr/bin:/bin
  # shellcheck disable=SC1090
  . "$SAFE_HOME/.profile"
  expect_equal "${PATH%%:*}" "$SPECIAL_BIN"
)
beampipe_persist_path "$SPECIAL_BIN" > /dev/null
if [ "$(grep -c 'Added by Beampipe installer' "$SAFE_HOME/.profile")" -ne 1 ]; then
  echo "special PATH line was duplicated" >&2
  exit 1
fi

SHELL=/usr/bin/fish
beampipe_persist_path "$SPECIAL_BIN" > "$SAFETY_TMP/fish-persist-output"
FISH_FILE="$SAFE_HOME/.config/fish/config.fish"
expect_equal "$(sed -n '1p' "$FISH_FILE")" "$(beampipe_fish_path_command "$SPECIAL_BIN")"
if command -v fish > /dev/null 2>&1; then
  fish -n "$FISH_FILE"
  fish -c 'source "$argv[1]"; test "$PATH[1]" = "$argv[2]"' "$FISH_FILE" "$SPECIAL_BIN"
fi

BAD_PATH=$(printf 'bad\npath')
REJECT_HOME="$SAFETY_TMP/rejected-home"
mkdir -p "$REJECT_HOME"
if HOME="$REJECT_HOME" beampipe_persist_path "$BAD_PATH" > "$SAFETY_TMP/bad-path" 2>&1; then
  echo "multiline PATH directory was accepted" >&2
  exit 1
fi
if [ -e "$REJECT_HOME/.profile" ]; then
  echo "multiline PATH directory changed a startup file" >&2
  exit 1
fi
if beampipe_validate_setup_args --project-config "$BAD_PATH" > "$SAFETY_TMP/bad-arg" 2>&1; then
  echo "multiline setup argument was accepted" >&2
  exit 1
fi
for marker in "$PATH_SUB_MARKER" "$PATH_TICK_MARKER" "$PATH_SEPARATOR_MARKER"; do
  if [ -e "$marker" ]; then
    echo "PATH persistence executed path content" >&2
    exit 1
  fi
done

echo "install progress and recovery output ok"
