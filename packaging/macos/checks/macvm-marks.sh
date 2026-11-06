#!/usr/bin/env bash
# The paddock marks on the windows of a macOS guest session, `paddock
# freeze` of the session, a screen that goes away during a session, and a
# quit of a frozen session, checked on a Mac.
#
# Run this script on an Apple silicon Mac. It builds the
# worktree, signs `paddock-vm` with its entitlement, starts one session of
# a scratch profile in a temporary data folder (never the real paddock
# data), launches TextEdit in the guest with two documents, and runs every
# check through `paddock windows --json`, `paddock freeze`, `paddock
# thaw`, `paddock app stop`, the control socket of the shim, the session
# record and the stamp of the profile. Each check prints `PASS name`,
# `FAIL name: reason` or `SKIP name: reason`; the script exits 0 when
# every check that ran passed.
#
# Usage:
#   packaging/macos/checks/macvm-marks.sh build         # compile, clippy, tests
#   packaging/macos/checks/macvm-marks.sh all           # build plus every check
#   packaging/macos/checks/macvm-marks.sh freeze        # one check by name
#
# What it needs: a sealed golden (`paddock macvm setup`) named by
# PADDOCK_MACVM_GOLDEN, the Xcode command line tools (`swift` reads the
# screens and posts Cmd-N), and for `screen` a second screen that the
# script may turn off: MARKS_SCREEN_ID is its id in `displayplacer list`
# (`brew install displayplacer`). The script turns that screen on again
# at the end. Without the golden, only `build` runs and the other checks
# print SKIP. To check the marks on ChatGPT instead of TextEdit, set
# MARKS_APP=/Applications/ChatGPT.app in the guest (the app must be in the
# guest already, for example after macvm-chatgpt.sh).
#
# What proves each check:
#   build        `cargo build`, `cargo clippy -D warnings` and `cargo test
#                mark freeze macvm::limits macvm::pause macvm::screens
#                macvm::host_running readme` exit 0 on macOS.
#   marks        `paddock windows --json` lists both session windows, each
#                with `sandbox` = `macvm-marks`, `kind` = `app`, `tone` =
#                `sage` and a `badge` that contains ` · vm · `, and the
#                marker process (`paddock mark`) runs. Look at the two
#                windows: each has the sage frame, and the menu of the
#                shield in the menu bar lists both as `marks (paddock) · ...
#                · vm · ...`.
#   freeze       `paddock freeze macvm-marks`: `health.t` of the session record
#                stops: it is the same after 1 s and after 4 s (the guest
#                sends one every 2 s), and `paddock ps` shows the session
#                frozen. `paddock thaw macvm-marks`: `health.t` moves again
#                within 3 s, and events.jsonl has the `freeze` and `thaw`
#                lines.
#   screen       with the second screen of MARKS_SCREEN_ID: `window move` puts
#                the second document onto it, then `displayplacer` turns the
#                screen off; within 2 s the window's center is on the main
#                screen in `window list`, the session record's badge is
#                `display changed, restart for full size`, and events.jsonl
#                has the `screens` line.
#   quit-frozen  `paddock freeze macvm-marks`, then `paddock app stop marks`:
#                the stop prints the thaw line, the shim ends within 60 s, and
#                the stamp of the profile has `"clean": true`.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-marks.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profile="$root/profiles.noindex/marks/default"
events="$profile/logs/events.jsonl"
record="$root/sessions/marks--default.json"
id=macvm-marks
app=${MARKS_APP:-/System/Applications/TextEdit.app}
screen_off=""

cleanup() {
  paddock thaw "$id" >/dev/null 2>&1
  paddock app stop marks >/dev/null 2>&1
  sleep 2
  pgrep -f "paddock-vm app --profile $profile" | xargs -r kill -9 2>/dev/null
  if [ -n "$screen_off" ]; then displayplacer "id:$screen_off enabled:true" >/dev/null 2>&1; fi
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

cat >"$tmp/marks.swift" <<'EOF'
import Foundation
import AppKit
let a = CommandLine.arguments
switch a[1] {
case "key":
  // key CODE cmd: a key down and up with Command.
  let code = CGKeyCode(UInt16(a[2])!)
  let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)
  let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)
  down?.flags = .maskCommand; up?.flags = .maskCommand
  down?.post(tap: .cghidEventTap); usleep(30_000); up?.post(tap: .cghidEventTap)
case "screens":
  // screens: one line per screen, `X Y W H`, AppKit coordinates, main first.
  for s in NSScreen.screens { let f = s.frame; print("\(Int(f.origin.x)) \(Int(f.origin.y)) \(Int(f.size.width)) \(Int(f.size.height))") }
default:
  print("unknown"); exit(2)
}
EOF
helper() { swift "$tmp/marks.swift" "$@"; }

build() {
  if ! cargo build --release --bin paddock --bin paddock-vm >"$tmp/build.log" 2>&1; then
    fail build "cargo build: $(tail -5 "$tmp/build.log" | tr '\n' ' ')"; return
  fi
  if ! codesign --force --sign - --entitlements packaging/macos/paddock-vm.entitlements "$bin/paddock-vm" >>"$tmp/build.log" 2>&1; then
    fail build "codesign of paddock-vm failed"; return
  fi
  if ! cargo clippy --all-targets -- -D warnings >"$tmp/clippy.log" 2>&1; then
    fail build "clippy: $(grep -m1 -A3 '^error' "$tmp/clippy.log" | tr '\n' ' ')"; return
  fi
  for t in mark freeze macvm::limits macvm::pause macvm::screens macvm::host_running readme; do
    if ! cargo test "$t" >>"$tmp/test.log" 2>&1; then
      fail build "cargo test $t: $(grep -E 'FAILED|panicked' "$tmp/test.log" | head -3 | tr '\n' ' ')"; return
    fi
  done
  pass build
}

golden_ready() {
  if [ -z "${PADDOCK_MACVM_GOLDEN:-}" ] || [ ! -f "$PADDOCK_MACVM_GOLDEN/manifest.toml" ]; then
    return 1
  fi
  if [ ! -d "$root/golden.noindex" ]; then
    mkdir -p "$root"
    cp -c -R "$PADDOCK_MACVM_GOLDEN" "$root/golden.noindex" || return 1
    chmod -R u+w "$root/golden.noindex"
  fi
  return 0
}

control() {
  local sock
  sock=$(sed -n 's/.*"control": *"\([^"]*\)".*/\1/p' "$record" | head -1)
  [ -n "$sock" ] || { echo "error no control socket in $record"; return 1; }
  printf '%s\n' "$1" | nc -U "$sock"
}
wins() { control "window list" | sed -n 's/^ok //p'; }
field() { printf '%s' "$1" | tr '{' '\n' | sed -n "s/.*\"$2\":\([^,}]*\).*/\1/p" | tr -d '"'; }
shim_pid() { sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$record" | head -1; }
# health_t: the time of the last health in the session record (the keys
# of the record are in sorted order, so `t` is not the first of `health`).
health_t() { tr -d '\n ' <"$record" | sed -n 's/.*"health":{[^}]*"t":\([0-9]*\).*/\1/p' | head -1; }
wait_event() {
  local i=0 n; n=$(awk "BEGIN{print int($3 * 10)}")
  while [ "$i" -lt "$n" ]; do
    grep -q "\"kind\":\"$1\".*$2" "$events" 2>/dev/null && return 0
    sleep 0.1; i=$((i + 1))
  done
  return 1
}

# The session with two windows of the app. Sets $key and $other.
session_up() {
  if [ -f "$record" ] && kill -0 "$(shim_pid)" 2>/dev/null; then return 0; fi
  rm -f "$record"
  paddock app run marks --backend macvm --network none >"$tmp/marks.out" 2>&1 &
  wait_event hello agent 90 || { echo "no hello within 90 s (see $profile/logs/)"; return 1; }
  control "bench launch $app" | grep -q '^ok' || { echo "bench launch refused"; return 1; }
  local i=0
  while [ "$i" -lt 150 ]; do
    [ "$(field "$(wins)" id | wc -l)" -ge 1 ] && break
    sleep 0.1; i=$((i + 1))
  done
  local first; first=$(field "$(wins)" id | head -1)
  [ -n "$first" ] || { echo "the app showed no window in 15 s"; return 1; }
  control "window raise $first" >/dev/null
  sleep 0.5
  helper key 45 # Cmd-N: a second window
  i=0
  while [ "$i" -lt 50 ]; do
    [ "$(field "$(wins)" id | wc -l)" -ge 2 ] && break
    sleep 0.1; i=$((i + 1))
  done
  key=$(field "$(wins)" id | head -1)
  other=$(field "$(wins)" id | grep -v "^$key$" | head -1)
  [ -n "$key" ] && [ -n "$other" ] || { echo "no two windows: $(wins)"; return 1; }
  return 0
}

check_marks() {
  local json i=0
  while [ "$i" -lt 30 ]; do
    json=$(paddock windows --json 2>/dev/null | tr -d '\n')
    [ "$(printf '%s' "$json" | grep -o "\"sandbox\": *\"$id\"" | wc -l)" -ge 2 ] && break
    sleep 0.5; i=$((i + 1))
  done
  local rows; rows=$(printf '%s' "$json" | tr '{' '\n' | grep "\"sandbox\": *\"$id\"")
  local n; n=$(printf '%s\n' "$rows" | grep -c . )
  [ "$n" -ge 2 ] || { fail marks "paddock windows --json lists $n windows of $id, not 2: $json"; return; }
  if printf '%s\n' "$rows" | grep -v '"tone": *"sage"' | grep -q .; then
    fail marks "a window is not sage: $rows"; return
  fi
  if printf '%s\n' "$rows" | grep -v '"badge": *"[^"]* · vm · ' | grep -q .; then
    fail marks "a badge lacks vm: $rows"; return
  fi
  pgrep -f "paddock mark" >/dev/null || { fail marks "the marker does not run"; return; }
  echo "  rows: $rows"
  pass marks
}

check_freeze() {
  paddock freeze "$id" --reason marks >"$tmp/freeze.out" 2>&1 || { fail freeze "paddock freeze: $(cat "$tmp/freeze.out")"; return; }
  sleep 1
  local t1; t1=$(health_t)
  sleep 3
  local t2; t2=$(health_t)
  [ -n "$t1" ] && [ "$t1" = "$t2" ] || { fail freeze "health moved while frozen: $t1 then $t2"; paddock thaw "$id" >/dev/null 2>&1; return; }
  paddock ps 2>/dev/null | grep "$id" | grep -q frozen || { fail freeze "paddock ps does not show $id frozen"; paddock thaw "$id" >/dev/null 2>&1; return; }
  paddock thaw "$id" >"$tmp/thaw.out" 2>&1 || { fail freeze "paddock thaw: $(cat "$tmp/thaw.out")"; return; }
  local i=0 t3=""
  while [ "$i" -lt 30 ]; do
    t3=$(health_t)
    [ -n "$t3" ] && [ "$t3" != "$t2" ] && break
    sleep 0.1; i=$((i + 1))
  done
  [ "$t3" != "$t2" ] || { fail freeze "no health within 3 s after the thaw (still $t2)"; return; }
  wait_event freeze paused 1 && wait_event thaw "runs again" 1 || { fail freeze "events.jsonl lacks the freeze or thaw line"; return; }
  pass freeze
}

check_screen() {
  [ -n "${MARKS_SCREEN_ID:-}" ] || { skip screen "set MARKS_SCREEN_ID to the displayplacer id of a second screen"; return; }
  command -v displayplacer >/dev/null || { skip screen "displayplacer is not installed"; return; }
  local screens; screens=$(helper screens)
  [ "$(printf '%s\n' "$screens" | wc -l)" -ge 2 ] || { skip screen "one screen only"; return; }
  local mx my mw mh; read -r mx my mw mh <<<"$(printf '%s\n' "$screens" | sed -n 1p)"
  local sx sy sw sh; read -r sx sy sw sh <<<"$(printf '%s\n' "$screens" | sed -n 2p)"
  # Guest coordinates: the union's top left is the origin, y down.
  local ux uy_top; ux=$(printf '%s\n' "$screens" | awk 'NR==1{m=$1} $1<m{m=$1} END{print m}')
  uy_top=$(printf '%s\n' "$screens" | awk 'NR==1{m=$2+$4} $2+$4>m{m=$2+$4} END{print m}')
  control "window move $other $((sx - ux + 100)) $((uy_top - (sy + sh) + 100)) 500 300" | grep -q '^ok' || { fail screen "window move refused"; return; }
  sleep 1
  displayplacer "id:$MARKS_SCREEN_ID enabled:false" >"$tmp/dp.out" 2>&1 || { fail screen "displayplacer: $(cat "$tmp/dp.out")"; return; }
  screen_off=$MARKS_SCREEN_ID
  # The main screen in guest coordinates.
  local gx0 gy0; gx0=$((mx - ux)); gy0=$((uy_top - (my + mh)))
  local i=0 ok=""
  while [ "$i" -lt 20 ]; do
    local entry x y w h
    entry=$(wins | tr '{' '\n' | grep "\"id\":$other[,}]")
    x=$(field "$entry" x | cut -d. -f1); y=$(field "$entry" y | cut -d. -f1)
    w=$(field "$entry" w | cut -d. -f1); h=$(field "$entry" h | cut -d. -f1)
    local cx cy; cx=$(( ${x:-0} + ${w:-0} / 2 )); cy=$(( ${y:-0} + ${h:-0} / 2 ))
    if [ "$cx" -ge "$gx0" ] && [ "$cx" -le "$((gx0 + mw))" ] && [ "$cy" -ge "$gy0" ] && [ "$cy" -le "$((gy0 + mh))" ]; then ok=1; break; fi
    sleep 0.1; i=$((i + 1))
  done
  displayplacer "id:$MARKS_SCREEN_ID enabled:true" >/dev/null 2>&1 && screen_off=""
  [ -n "$ok" ] || { fail screen "the window is not on the main screen within 2 s: $entry"; return; }
  grep -q '"badge": *"display changed, restart for full size"' "$record" || { fail screen "the badge is not the display change"; return; }
  wait_event screens "screens changed" 1 || { fail screen "events.jsonl lacks the screens line"; return; }
  pass screen
}

check_quit_frozen() {
  paddock freeze "$id" --reason marks >/dev/null 2>&1 || { fail quit-frozen "paddock freeze failed"; return; }
  local pid; pid=$(shim_pid)
  paddock app stop marks >"$tmp/stop.out" 2>&1
  grep -q "thaws it first" "$tmp/stop.out" || { fail quit-frozen "no thaw line: $(cat "$tmp/stop.out")"; return; }
  local i=0
  while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 600 ]; do sleep 0.1; i=$((i + 1)); done
  kill -0 "$pid" 2>/dev/null && { fail quit-frozen "the shim $pid still runs after 60 s"; return; }
  tr -d '\n ' <"$profile/stamp.json" | grep -q '"clean":true' || { fail quit-frozen "the stamp is not clean: $(cat "$profile/stamp.json")"; return; }
  pass quit-frozen
}

needs_session() { case $1 in marks|freeze|screen|quit-frozen) return 0;; *) return 1;; esac; }

run_check() {
  local name=$1
  if [ "$name" = build ]; then build; return; fi
  if ! golden_ready; then skip "$name" "no golden: set PADDOCK_MACVM_GOLDEN to a sealed golden"; return; fi
  if needs_session "$name"; then
    session_up >"$tmp/up.log" 2>&1 || { fail "$name" "no session: $(cat "$tmp/up.log")"; return; }
  fi
  "check_${name//-/_}"
}

case $what in
  all) for c in build marks freeze screen quit-frozen; do run_check "$c"; done ;;
  build|marks|freeze|screen|quit-frozen) run_check "$what" ;;
  *) echo "unknown check $what"; exit 2 ;;
esac
exit $((fails > 0))
