#!/bin/sh
# Static/command-shape regression for checkout setup. No Docker daemon is used.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
calls="$tmp/docker-calls"

cat >"$tmp/docker" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >>"$BEAMPIPE_DOCKER_CALLS"
case "$*" in
  "compose pull api"|"compose run "*) exit 0 ;;
  *)
    echo "unexpected docker invocation: $*" >&2
    exit 1
    ;;
esac
SH
chmod +x "$tmp/docker"

BEAMPIPE_DOCKER_CALLS="$calls" \
PATH="$tmp:$PATH" \
sh "$root/deploy/setup-docker.sh" --yes --skip-admin --skip-upload

if [ "$(wc -l <"$calls" | tr -d ' ')" -ne 2 ]; then
  echo "expected image pull and setup container invocations" >&2
  cat "$calls" >&2
  exit 1
fi

setup_call=$(tail -n 1 "$calls")
case "$setup_call" in
  *" -v $root:/checkout -w /checkout api setup --runtime docker --no-start --directory /checkout --yes --skip-admin --skip-upload") ;;
  *)
    echo "checkout setup did not select /checkout as the installation directory" >&2
    echo "$setup_call" >&2
    exit 1
    ;;
esac

echo "checkout setup directory forwarding ok"
