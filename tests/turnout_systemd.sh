#!/usr/bin/env bash
# Host acceptance: real user systemd, isolated state and uniquely named units.
set -euo pipefail
binary=$(realpath "${1:?pass the paddock binary}")
root=$(mktemp -d)
tag="test-$(date +%s)-$$"
pass="${tag}-pass"
fail="${tag}-fail"
timer="paddock-turnout-${tag}-timer"
cleanup() {
  systemctl --user stop "$timer.timer" "$timer.service" "paddock-turnout-work-$pass.service" "paddock-turnout-work-$fail.service" >/dev/null 2>&1 || true
  systemctl --user reset-failed "$timer.service" "paddock-turnout-work-$pass.service" "paddock-turnout-work-$fail.service" >/dev/null 2>&1 || true
  rm -rf "$root"
}
trap cleanup EXIT
export HOME="$root/home" XDG_CONFIG_HOME="$root/config" XDG_DATA_HOME="$root/data" XDG_STATE_HOME="$root/state" XDG_CACHE_HOME="$root/cache"
mkdir -p "$HOME" "$XDG_CONFIG_HOME/paddock" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"
cat > "$XDG_CONFIG_HOME/paddock/config.toml" <<CONFIG
[turnout.$fail]
harness = 'none'
trigger = 'timer'
schedule = 'daily'
check = 'exit 1'
[turnout.$pass]
harness = 'none'
trigger = 'timer'
schedule = 'daily'
check = 'echo work'
max_runtime = '30s'
max_mem = '256M'
CONFIG
cd "$root"
"$binary" turnout run "$fail"
grep -q 'not admitted' "$XDG_STATE_HOME/paddock/turnout/$fail.jsonl"
grep -q '"mail_out":null' "$XDG_STATE_HOME/paddock/turnout/$fail.jsonl"
systemd-run --user --quiet --on-active=2s --timer-property=AccuracySec=1s --unit="$timer" \
  --property="WorkingDirectory=$root" --setenv="HOME=$HOME" --setenv="XDG_CONFIG_HOME=$XDG_CONFIG_HOME" \
  --setenv="XDG_DATA_HOME=$XDG_DATA_HOME" --setenv="XDG_STATE_HOME=$XDG_STATE_HOME" \
  --setenv="XDG_CACHE_HOME=$XDG_CACHE_HOME" --setenv="PATH=$PATH" \
  "$binary" turnout run "$pass"
for _ in $(seq 1 30); do
  [ -f "$XDG_STATE_HOME/paddock/turnout/$pass.jsonl" ] && break
  sleep 1
done
grep -q 'completed' "$XDG_STATE_HOME/paddock/turnout/$pass.jsonl"
"$binary" mail ls "turnout-$pass" | grep -q 'completed'
printf 'Real timer, transient service, check gate, mail and health log passed.\n'
