#!/usr/bin/env bash
# Shared by the `check-NN-*.sh` scripts of the security checklist: a
# temporary data folder with a clone of the sealed golden, scratch
# profiles that run TextEdit, and the helpers that start, watch and stop
# a session. Source it; do not run it.
#
# The caller sets `item` (the number of the row) before `ok` or `fail`.
# `ok` prints `ok NN`, `fail` prints `FAIL NN: reason`. Every check exits
# 0 for `ok` and 1 for `FAIL`, so `security.sh` sums them.
#
# Needs: an Apple silicon Mac, `cargo build --release` done (or `bin` set
# to a folder with `paddock` and `paddock-vm`), and PADDOCK_MACVM_GOLDEN
# pointing at a sealed golden folder (`paddock macvm setup`).

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
cd "$here" || exit 1
bin=${bin:-$here/target/release}
export PATH="$bin:$PATH"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-sec.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
root="$XDG_DATA_HOME/paddock/macvm"
profiles="$root/profiles.noindex"
golden="$root/golden.noindex"
guest_home=/Volumes/paddock/home
item=${item:-00}

ok() { echo "ok $item"; cleanup; exit 0; }
fail() { echo "FAIL $item: $1"; cleanup; exit 1; }
note() { echo "  $item: $1"; }

cleanup() {
  trap - EXIT
  for n in $(ls "$profiles" 2>/dev/null); do paddock app stop "$n" >/dev/null 2>&1 || true; done
  sleep 2
  pgrep -f "paddock-vm app --profile $root" | xargs kill -9 2>/dev/null
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

# A clone of the sealed golden in the temporary data folder (APFS, no
# space). The clone keeps the 0444 mode; the `uchg` flag is set again,
# because `cp` does not copy it.
golden_ready() {
  if [ -z "${PADDOCK_MACVM_GOLDEN:-}" ] || [ ! -f "$PADDOCK_MACVM_GOLDEN/manifest.toml" ]; then
    fail "PADDOCK_MACVM_GOLDEN does not name a sealed golden (run paddock macvm setup first)"
  fi
  if [ ! -d "$golden" ]; then
    mkdir -p "$root"
    cp -c -R "$PADDOCK_MACVM_GOLDEN" "$golden" || fail "cannot clone the golden into $root"
    chmod 0444 "$golden/disk.img" "$golden/aux.img" 2>/dev/null
    chflags uchg "$golden/disk.img" "$golden/aux.img" 2>/dev/null
  fi
}

# start NAME [args]: a session in the background; prints its pid.
# No check of this script waits for a person: a session runs with no
# network (--network none), so no firewall question can come up.
start() {
  local name=$1; shift
  paddock app run "$name" --backend macvm --network none "$@" >"$tmp/$name.out" 2>&1 &
  echo $!
}
events() { echo "$profiles/$1/default/logs/events.jsonl"; }
# wait_event PROFILE KIND SECONDS: true when the event log of the profile
# gets a line of that kind within the seconds.
wait_event() {
  local log; log=$(events "$1"); local kind=$2 secs=$3 i=0
  while [ "$i" -lt "$secs" ]; do
    grep -q "\"kind\":\"$kind\"" "$log" 2>/dev/null && return 0
    sleep 1; i=$((i + 1))
  done
  return 1
}
shim_pid() { sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$root/sessions/$1--default.json" 2>/dev/null | head -1; }
control() { paddock macvm control "$1" "${@:2}"; }
# guest NAME CMD...: run a command in the guest through the agent's bench
# launch door, and print its output from the downloads share.
guest() {
  local name=$1; shift
  local out="$profiles/$name/default/downloads/.out-$$"
  rm -f "$out"
  control "$name" bench launch "/bin/sh -c '$* > /Volumes/My\\ Shared\\ Files/downloads/.out-$$ 2>&1; sync'" >/dev/null
  local i=0
  while [ "$i" -lt 20 ] && [ ! -f "$out" ]; do sleep 0.5; i=$((i + 1)); done
  cat "$out" 2>/dev/null
}
# session_up NAME: start it and wait for the hello and the app launch.
session_up() {
  start "$1" >/dev/null
  wait_event "$1" hello 120 || return 1
  wait_event "$1" launch 60 || return 1
  return 0
}
stop() { paddock app stop "$1" >/dev/null 2>&1; }
# mode PATH: the octal mode, `0600`.
mode() { stat -f %Mp%Lp "$1" 2>/dev/null; }
# flags PATH: the flags as `ls -lO` shows them, `uchg` or `-`.
flags() { ls -lO "$1" 2>/dev/null | awk '{print $5}'; }
