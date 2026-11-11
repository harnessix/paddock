#!/usr/bin/env bash
# Upgrades and migration, checked on a Mac. The agent self-update
# through the `agent` share, its rollback by the `launch` script, `paddock
# macvm update`, `paddock macvm rollback` and the host build migration.
#
# Run this script on an Apple silicon Mac with a sealed
# golden (`paddock macvm setup`) whose agent is the one of this
# worktree. Every check runs in a temporary data folder with a clone of
# that golden (APFS, no space), so the real profiles are never touched.
# Each check prints one line, `PASS name` or `FAIL name: reason`, and the
# script exits 0 when every check that ran passed.
#
# Usage:
#   PADDOCK_MACVM_GOLDEN=~/.local/share/paddock/macvm/golden.noindex \
#     packaging/macos/checks/macvm-upgrade.sh all
#   packaging/macos/checks/macvm-upgrade.sh build        # the three builds only
#   packaging/macos/checks/macvm-upgrade.sh agent-update # one check
#
# The three builds: the version is compiled in, so the check builds the
# worktree three times with PADDOCK_MACVM_VERSION set (the agent and the
# host of one build report the same version): V0 (the version of the
# golden's agent, this worktree as it is), V1 (V0 plus `-upga`, the
# "bumped minor") and V2 (V0 plus `-upgb`). Each build lands in its own
# target folder, and the check points PATH at one of them per step.
#
# What proves each check (the lines of this script and the files it reads):
#   build            cargo build of paddock, paddock-vm and paddock-guest
#                    for V0, V1 and V2; paddock-vm signed with its
#                    entitlement; paddock-guest signed; the .sha256 files
#                    match the binaries.
#   agent-update     a session of V1 against the golden's agent V0: the
#                    events log has `hello` of V0, then `agent update
#                    from V0 to V1`, then a second `hello` of V1 within
#                    5 s of the first, and `agent update done`. The
#                    window list of TextEdit is the same before and after
#                    (ids and frames in the `windows` events: the app
#                    never closed).
#   agent-downgrade  the same session profile with V0 again: `agent
#                    update from V1 to V0 (a downgrade...)`, a `hello` of
#                    V0 within 5 s.
#   agent-bad        V2 with a paddock-guest that exits 1 at once in its
#                    agent folder (the hash file matches that script):
#                    `agent update from V0 to V2`, then within 15 s a
#                    `hello` of V0 again (prev/ became current/) and the
#                    event `agent rollback: the guest runs V0 again`. No
#                    second update request in the log.
#   golden-update    `paddock macvm update --ipsw $PADDOCK_MACVM_IPSW
#                    --force --yes` (the same image is fine with --force)
#                    builds golden.new and swaps: golden.prev exists with
#                    the old manifest, `macvm status` has the `prev` line.
#                    The next start of the profile has the event `cold:
#                    the golden changed`, a `backups/pre-golden-*.img`
#                    appears, the marker file that the check wrote into
#                    the guest home before the update is still on the data
#                    disk (hdiutil attach of data.img after the session),
#                    and no `copy` event (the app copy into the guest is
#                    not repeated).
#   golden-rollback  `paddock macvm rollback --yes`: golden.prev is gone,
#                    the next start has `cold: the golden changed` again,
#                    the marker is still on the data disk.
#   host-build       stamp.json edited to another host_build: the next
#                    start prints `the saved state is from macOS <old>,
#                    the host runs <new>: cold boot` with both builds, the
#                    marker is on the data disk.
#
# The guided step of the golden-update check is manual (Setup Assistant
# in the paddock window, about three minutes), as in `paddock macvm
# setup`. PADDOCK_MACVM_IPSW names the restore image; without it the
# golden checks print SKIP.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-upgrade.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
golden="$root/golden.noindex"
profile="$root/profiles.noindex/TextEdit/default"
events="$profile/logs/events.jsonl"
app=TextEdit
v0=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
v1="$v0-upga"
v2="$v0-upgb"

cleanup() {
  paddock app stop "$app" >/dev/null 2>&1 || true
  sleep 2
  pgrep -f "paddock-vm app --profile $root" | xargs -r kill -9 2>/dev/null
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

# use V: PATH at the build of version V.
use() { export PATH="$here/target/upgrade-$1/release:$PATH"; }
# build_one TAG VERSION: one build with the version compiled in.
build_one() {
  local dir="$here/target/upgrade-$1"
  if ! PADDOCK_MACVM_VERSION="$2" cargo build --release --target-dir "$dir" --bin paddock --bin paddock-vm --bin paddock-guest >"$tmp/build-$1.log" 2>&1; then
    fail build "cargo build $1: $(tail -5 "$tmp/build-$1.log" | tr '\n' ' ')"; return 1
  fi
  codesign --force --sign - --entitlements packaging/macos/paddock-vm.entitlements "$dir/release/paddock-vm" >>"$tmp/build-$1.log" 2>&1 || { fail build "codesign paddock-vm $1"; return 1; }
  codesign --force --sign - "$dir/release/paddock-guest" >>"$tmp/build-$1.log" 2>&1 || { fail build "codesign paddock-guest $1"; return 1; }
  (cd "$dir/release" && shasum -a 256 paddock-guest | awk '{print $1}' > paddock-guest.sha256)
  local got; got=$(shasum -a 256 "$dir/release/paddock-guest" | awk '{print $1}')
  [ "$got" = "$(cat "$dir/release/paddock-guest.sha256")" ] || { fail build "hash file of $1"; return 1; }
  return 0
}
build() {
  build_one v0 "$v0" && build_one v1 "$v1" && build_one v2 "$v2" || return 1
  pass build
}

# The golden clone into the temporary data folder, once.
golden_ready() {
  [ -d "$golden" ] && return 0
  if [ -z "${PADDOCK_MACVM_GOLDEN:-}" ] || [ ! -f "$PADDOCK_MACVM_GOLDEN/manifest.toml" ]; then
    return 1
  fi
  mkdir -p "$root"
  cp -c -R "$PADDOCK_MACVM_GOLDEN" "$golden" 2>/dev/null || cp -R "$PADDOCK_MACVM_GOLDEN" "$golden"
  chflags -R nouchg "$golden"; chmod -R u+w "$golden"
  chmod 444 "$golden/disk.img" "$golden/aux.img"; chflags uchg "$golden/disk.img" "$golden/aux.img"
}

# run_session: one session to its hello and back. Prints the start line.
# wait_event PATTERN SECS: wait for a line of events.jsonl.
wait_event() {
  local i=0
  while [ "$i" -lt "$2" ]; do
    grep -q "$1" "$events" 2>/dev/null && return 0
    sleep 1; i=$((i + 1))
  done
  return 1
}
start_session() {
  : > "$tmp/run.log"
  paddock app run "$app" --backend macvm --network none >"$tmp/run.log" 2>&1 &
  echo $! > "$tmp/run.pid"
}
stop_session() {
  paddock app stop "$app" >/dev/null 2>&1 || true
  wait "$(cat "$tmp/run.pid")" 2>/dev/null || true
}
# hello_times VERSION: the epoch seconds of each hello of that version.
hello_times() {
  grep '"kind":"hello"' "$events" | grep "agent $1," | sed -n 's/.*"t":"\([^"]*\)".*/\1/p' | while read -r t; do date -j -f "%Y-%m-%dT%H:%M:%SZ" "$t" +%s; done
}
first_hello() { hello_times "$1" | head -1; }
last_hello() { hello_times "$1" | tail -1; }
windows_before_after_equal() {
  # The window events of TextEdit before the update and after the new
  # hello name the same ids and frames.
  local before after
  before=$(grep '"kind":"windows"' "$events" | awk -v u="$(grep -n 'agent update from' "$events" | head -1 | cut -d: -f1)" 'NR<u' | tail -1 | sed 's/.*"t":"[^"]*",//')
  after=$(grep '"kind":"windows"' "$events" | awk -v u="$(grep -n 'agent update done' "$events" | head -1 | cut -d: -f1)" 'NR>u' | head -1 | sed 's/.*"t":"[^"]*",//')
  [ -n "$before" ] && [ "$before" = "$after" ]
}

agent_update() {
  golden_ready || { skip agent-update "PADDOCK_MACVM_GOLDEN is not a sealed golden"; return; }
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$profile" 2>/dev/null
  rm -rf "$profile"
  use v1
  start_session
  if ! wait_event "agent $v0," 90; then fail agent-update "no hello of $v0 in 90 s: $(tail -3 "$tmp/run.log" | tr '\n' ' ')"; stop_session; return; fi
  if ! wait_event "agent $v1," 20; then fail agent-update "no hello of $v1: $(grep -c . "$events") events, $(grep agent "$events" | tail -3 | tr '\n' ' ')"; stop_session; return; fi
  stop_session
  local t0 t1
  t0=$(first_hello "$v0"); t1=$(first_hello "$v1")
  if ! grep -q "agent update from $v0 to $v1" "$events"; then fail agent-update "no 'agent update' event"; return; fi
  if [ $((t1 - t0)) -gt 5 ]; then fail agent-update "the new hello came $((t1 - t0)) s after the first"; return; fi
  if [ "$(grep -c 'agent update from' "$events")" != 1 ]; then fail agent-update "more than one update event"; return; fi
  if ! grep -q "agent update done: $v1" "$events"; then fail agent-update "no 'agent update done' event"; return; fi
  if grep -q '"kind":"windows"' "$events" && ! windows_before_after_equal; then fail agent-update "the window list changed across the update"; return; fi
  pass agent-update
}

agent_downgrade() {
  golden_ready || { skip agent-downgrade "no golden"; return; }
  [ -f "$events" ] || { skip agent-downgrade "run agent-update first"; return; }
  use v0
  : > "$events"
  start_session
  if ! wait_event "agent $v1," 90; then fail agent-downgrade "no hello of $v1"; stop_session; return; fi
  if ! wait_event "agent $v0," 20; then fail agent-downgrade "no hello of $v0 after the downgrade"; stop_session; return; fi
  stop_session
  local t0 t1
  t1=$(first_hello "$v1"); t0=$(first_hello "$v0")
  if ! grep -q "agent update from $v1 to $v0 (a downgrade" "$events"; then fail agent-downgrade "no downgrade event: $(grep 'agent update' "$events" | tr '\n' ' ')"; return; fi
  if [ $((t0 - t1)) -gt 5 ]; then fail agent-downgrade "the new hello came $((t0 - t1)) s after the first"; return; fi
  pass agent-downgrade
}

agent_bad() {
  golden_ready || { skip agent-bad "no golden"; return; }
  [ -f "$events" ] || { skip agent-bad "run agent-update first"; return; }
  # V2 with a bad agent in its install folder: a script that exits 1.
  local dir="$here/target/upgrade-v2/release"
  cp "$dir/paddock-guest" "$tmp/paddock-guest.v2"
  printf '#!/bin/sh\nexit 1\n' > "$dir/paddock-guest"; chmod 755 "$dir/paddock-guest"
  (cd "$dir" && shasum -a 256 paddock-guest | awk '{print $1}' > paddock-guest.sha256)
  rm -rf "$root/agent"
  use v2
  : > "$events"
  start_session
  local ok=1
  if ! wait_event "agent $v0," 90; then fail agent-bad "no hello of $v0"; ok=0; fi
  if [ "$ok" = 1 ] && ! wait_event "agent rollback: the guest runs $v0 again" 30; then fail agent-bad "no 'agent rollback' event: $(grep agent "$events" | tail -4 | tr '\n' ' ')"; ok=0; fi
  stop_session
  cp "$tmp/paddock-guest.v2" "$dir/paddock-guest"
  (cd "$dir" && shasum -a 256 paddock-guest | awk '{print $1}' > paddock-guest.sha256)
  [ "$ok" = 1 ] || return
  local t0 t0b
  t0=$(first_hello "$v0"); t0b=$(last_hello "$v0")
  if [ "$(hello_times "$v0" | wc -l | tr -d ' ')" -lt 2 ]; then fail agent-bad "no second hello of $v0 (from prev/)"; return; fi
  if [ $((t0b - t0)) -gt 15 ]; then fail agent-bad "the hello from prev came $((t0b - t0)) s after the first"; return; fi
  if [ "$(grep -c 'agent update from' "$events")" != 1 ]; then fail agent-bad "the host sent a second update after the rollback"; return; fi
  pass agent-bad
}

# marker_write / marker_present: a file in the guest home on the data disk,
# written and read with hdiutil while no session runs.
marker_write() {
  local dev; dev=$(hdiutil attach -imagekey diskimage-class=CRawDiskImage -nobrowse -owners on "$profile/data.img" 2>/dev/null | awk '/Apple_APFS|apfs/ {print $1; exit}')
  local mnt; mnt=$(mount | awk -v d="$dev" '$1 ~ d {print $3; exit}')
  [ -n "$mnt" ] || { hdiutil detach "$dev" >/dev/null 2>&1; return 1; }
  local home; home=$(ls -d "$mnt"/Users/* 2>/dev/null | head -1)
  [ -n "$home" ] || home="$mnt"
  echo "upgrade $(date +%s)" | sudo tee "$home/.paddock-upgrade-marker" >/dev/null
  hdiutil detach "$dev" >/dev/null 2>&1
}
marker_present() {
  local dev; dev=$(hdiutil attach -imagekey diskimage-class=CRawDiskImage -nobrowse -readonly "$profile/data.img" 2>/dev/null | awk '/Apple_APFS|apfs/ {print $1; exit}')
  local mnt; mnt=$(mount | awk -v d="$dev" '$1 ~ d {print $3; exit}')
  local r=1
  [ -n "$mnt" ] && ls "$mnt"/Users/*/.paddock-upgrade-marker "$mnt/.paddock-upgrade-marker" >/dev/null 2>&1 && r=0
  hdiutil detach "$dev" >/dev/null 2>&1
  return $r
}

golden_update() {
  golden_ready || { skip golden-update "no golden"; return; }
  [ -n "${PADDOCK_MACVM_IPSW:-}" ] || { skip golden-update "PADDOCK_MACVM_IPSW is not set"; return; }
  use v0
  [ -f "$profile/data.img" ] || { start_session; wait_event '"kind":"hello"' 90; stop_session; }
  marker_write || { fail golden-update "cannot write the marker into data.img"; return; }
  if ! paddock macvm update --ipsw "$PADDOCK_MACVM_IPSW" --force --yes >"$tmp/update.log" 2>&1; then
    fail golden-update "paddock macvm update: $(tail -3 "$tmp/update.log" | tr '\n' ' ')"; return
  fi
  [ -d "$root/golden.prev.noindex" ] || { fail golden-update "no golden.prev after the update"; return; }
  paddock macvm status | grep -q '^  prev ' || { fail golden-update "status has no prev line"; return; }
  : > "$events"
  start_session
  wait_event '"kind":"hello"' 120 || { fail golden-update "no hello after the update: $(tail -3 "$tmp/run.log" | tr '\n' ' ')"; stop_session; return; }
  stop_session
  grep -q 'cold: the golden changed' "$events" || { fail golden-update "no 'cold: the golden changed' start: $(grep start "$events" | tr '\n' ' ')"; return; }
  ls "$profile"/backups/pre-golden-*.img >/dev/null 2>&1 || { fail golden-update "no pre-golden backup"; return; }
  grep -q '"kind":"copy"' "$events" && { fail golden-update "an app copy event after the golden update"; return; }
  marker_present || { fail golden-update "the marker in the guest home is gone"; return; }
  pass golden-update
}

golden_rollback() {
  [ -d "$root/golden.prev.noindex" ] || { skip golden-rollback "no golden.prev (run golden-update first)"; return; }
  use v0
  paddock macvm rollback --yes >"$tmp/rollback.log" 2>&1 || { fail golden-rollback "$(tail -2 "$tmp/rollback.log" | tr '\n' ' ')"; return; }
  [ -d "$root/golden.prev.noindex" ] && { fail golden-rollback "golden.prev still there"; return; }
  : > "$events"
  start_session
  wait_event '"kind":"hello"' 120 || { fail golden-rollback "no hello after the rollback"; stop_session; return; }
  stop_session
  grep -q 'cold: the golden changed' "$events" || { fail golden-rollback "no 'cold: the golden changed' start"; return; }
  marker_present || { fail golden-rollback "the marker is gone"; return; }
  pass golden-rollback
}

host_build() {
  golden_ready || { skip host-build "no golden"; return; }
  [ -f "$profile/stamp.json" ] || { skip host-build "no profile yet"; return; }
  use v0
  local old; old=$(sed -n 's/.*"host_build": *"\([^"]*\)".*/\1/p' "$profile/stamp.json" | head -1)
  sed -i '' "s/\"host_build\": *\"$old\"/\"host_build\": \"OLDBUILD\"/" "$profile/stamp.json"
  : > "$events"
  start_session
  wait_event '"kind":"hello"' 120 || { fail host-build "no hello: $(tail -3 "$tmp/run.log" | tr '\n' ' ')"; stop_session; return; }
  stop_session
  if ! grep -q "the saved state is from macOS OLDBUILD, the host runs $old: cold boot" "$tmp/run.log" "$events"; then
    fail host-build "no cold boot line with both builds: $(grep start "$events" | tr '\n' ' ')"; return
  fi
  marker_present || { fail host-build "the marker is gone"; return; }
  pass host-build
}

case "$what" in
  build) build ;;
  all) build && { agent_update; agent_downgrade; agent_bad; golden_update; golden_rollback; host_build; } ;;
  agent-update) build && agent_update ;;
  agent-downgrade) build && agent_downgrade ;;
  agent-bad) build && agent_bad ;;
  golden-update) build_one v0 "$v0" && golden_update ;;
  golden-rollback) build_one v0 "$v0" && golden_rollback ;;
  host-build) build_one v0 "$v0" && host_build ;;
  *) echo "unknown check $what" >&2; exit 2 ;;
esac
[ "$fails" = 0 ]
