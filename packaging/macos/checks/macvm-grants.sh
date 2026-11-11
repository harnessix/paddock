#!/usr/bin/env bash
# The live grants of an app of the macOS guest (`paddock app share`,
# `paddock app network`, `paddock app clipboard`), checked on a Mac.
#
# Run this script on an Apple silicon Mac with a sealed
# golden image (PADDOCK_MACVM_GOLDEN names it, as macvm-files.sh does).
# It starts one scratch session of TextEdit with --bench in a temporary
# data folder, in network mode egress. A command in the guest runs
# through the bench door of the control socket (`bench op` with
# `bench.spawn_child`), and it writes what it saw into the read-write
# folder `ws-1`, so the host reads the result. Each check prints one
# line: `PASS name`, `FAIL name: reason` or `SKIP name: reason`. The
# script exits non-zero on a FAIL.
#
# Usage:
#   packaging/macos/checks/macvm-grants.sh all
#   packaging/macos/checks/macvm-grants.sh add        # one check by name
#
# What proves each check:
#   build     cargo build, the codesign of paddock-vm, clippy, and the tests
#             of macvm::grants, macvm::gate, macvm::clipboard, guest:: and
#             the Mac tests of the share set and of lsof.
#   add       `paddock app share APP add B --json` says "applies": "live"
#             and names ws-2; within 2 s the guest lists the file of B
#             under /Volumes/My Shared Files/ws-2; a touch there fails (read
#             only by default) and the host folder has no new file.
#   mode      `share APP mode B rw` is live: a touch in ws-2 makes the file
#             on the host.
#   handles   a guest process holds a file of ws-2 open; within 15 s
#             `share APP list --json` shows "open_handles" >= 1 for B and a
#             "last_use"; `share APP mode B ro --json` answers
#             "reopen_needed": true and the row says "reopen-files" for
#             about 30 s; `share APP remove B` without --force fails with
#             "files open in" and the folder stays.
#   remove    `share APP remove B --force` is live: the guest no longer
#             lists ws-2.
#   refuse    `share APP add ~`, `add ~/Library` and `add` of the paddock
#             data folder fail with one line each, and grants.json does
#             not change.
#   network   `paddock app network APP none --json` is live; a curl in the
#             guest fails; `app status APP --json` says "network": {"mode":
#             "none"}; back with `network APP allowlist`.
#   clipboard `paddock app clipboard APP --to-app off --json` is live and
#             `app status APP --json` has "clipboard": {"to_app": false}.
#             With PADDOCK_GRANTS_LOOK=1 it asks you to paste into TextEdit
#             and to say whether nothing came.
#   status    `app status APP --json` has open_handles_per_share,
#             clipboard, devices (each "applies": "next-start") and at
#             least six changes, the newest first; logs/events.jsonl has a
#             "kind":"grant" line with a "grant" object for each.
#   persist   a folder added, the session stopped and started again with
#             no folder argument: `share APP list --json` shows it live.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-grants.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
export PADDOCK_MACVM_BENCH=1
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profiles="$root/profiles.noindex"
work="$HOME/.paddock-grants-work-$$"
mkdir -p "$work/a" "$work/b" "$work/c"
echo hello >"$work/b/in-b.txt"
app=textedit-grants
profile="$profiles/$app/default"
events="$profile/logs/events.jsonl"
shared="/Volumes/My Shared Files"

cleanup() {
  paddock app stop "$app" --wait 30 >/dev/null 2>&1
  sleep 1
  pgrep -f "paddock-vm app --profile $profiles" | xargs -r kill -9 2>/dev/null
  chflags -R nouchg "$tmp" "$work" 2>/dev/null
  rm -rf "$tmp" "$work"
}
trap cleanup EXIT

json() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

build() {
  if ! cargo build --release --bin paddock --bin paddock-vm --bin paddock-guest >"$tmp/build.log" 2>&1; then
    fail build "cargo build: $(tail -5 "$tmp/build.log" | tr '\n' ' ')"; return
  fi
  if ! codesign --force --sign - --entitlements packaging/macos/paddock-vm.entitlements "$bin/paddock-vm" >>"$tmp/build.log" 2>&1; then
    fail build "codesign of paddock-vm failed"; return
  fi
  if ! cargo clippy --all-targets -- -D warnings >"$tmp/clippy.log" 2>&1; then
    fail build "clippy: $(grep -m1 -A3 '^error' "$tmp/clippy.log" | tr '\n' ' ')"; return
  fi
  for t in macvm::grants macvm::gate macvm::clipboard macvm::vz_mac_config guest:: mark::panels; do
    if ! cargo test --lib "$t" >>"$tmp/test.log" 2>&1; then
      fail build "tests $t: $(grep -E 'FAILED|panicked' "$tmp/test.log" | head -3 | tr '\n' ' ')"; return
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
  mkdir -p "$XDG_CONFIG_HOME/paddock"
  cat >"$XDG_CONFIG_HOME/paddock/config.toml" <<CFG
[apps.$app]
bundle = "/System/Applications/TextEdit.app"
CFG
  return 0
}

wait_line() {
  local file=$1 text=$2 secs=$3 i=0
  while [ "$i" -lt "$((secs * 10))" ]; do
    grep -qF "$text" "$file" 2>/dev/null && return 0
    sleep 0.1; i=$((i + 1))
  done
  return 1
}

control() {
  python3 -c "import json,glob; print([json.load(open(f))['control'] for f in glob.glob('$root/sessions/*.json')][0])" 2>/dev/null
}

# Run a shell command in the guest; its output goes to ws-1/NAME on the
# host (the folder $work/a). Waits up to 10 s for the file.
guest() {
  local name=$1 cmd=$2 sock
  sock=$(control) || return 1
  rm -f "$work/a/$name"
  local argv
  argv=$(python3 -c 'import json,sys; print(json.dumps({"op":"bench.spawn_child","argv":["/bin/sh","-c",sys.argv[1]]}))' \
    "$cmd > \"$shared/ws-1/$name.part\" 2>&1; echo \"exit \$?\" >> \"$shared/ws-1/$name.part\"; mv \"$shared/ws-1/$name.part\" \"$shared/ws-1/$name\"")
  printf 'bench op %s\n' "$argv" | nc -U "$sock" >/dev/null 2>&1
  wait_line "$work/a/$name" "exit" 10
}

start_session() {
  paddock app run "$app" --backend macvm --bench --network egress "$@" >"$tmp/run.out" 2>&1 &
  wait_line "$events" '"kind":"hello"' 120
}

session() {
  if ! golden_ready; then skip "$1" "PADDOCK_MACVM_GOLDEN names no sealed golden"; return 1; fi
  if [ -z "$(control)" ]; then
    start_session "$work/a" || { fail "$1" "the session did not start: $(tail -3 "$tmp/run.out" | tr '\n' ' ')"; return 1; }
    sleep 3
  fi
  if ! guest probe "echo probe"; then
    skip "$1" "the bench door of the control socket ran nothing (needs a --bench session and nc -U)"; return 1
  fi
}

check_add() {
  session add || return
  paddock app share "$app" add "$work/b" --json >"$tmp/add.json" 2>&1 || { fail add "$(cat "$tmp/add.json")"; return; }
  [ "$(json 'd["change"]["applies"]' <"$tmp/add.json")" = live ] || { fail add "not live: $(cat "$tmp/add.json")"; return; }
  [ "$(json 'd["live"]["name"]' <"$tmp/add.json")" = ws-2 ] || { fail add "not ws-2: $(cat "$tmp/add.json")"; return; }
  sleep 2
  guest ls-b "ls \"$shared/ws-2\""
  grep -q in-b.txt "$work/a/ls-b" || { fail add "the guest does not list ws-2: $(cat "$work/a/ls-b")"; return; }
  guest touch-ro "touch \"$shared/ws-2/x\""
  [ -e "$work/b/x" ] && { fail add "a touch in the read-only ws-2 made a file"; return; }
  pass add
}

check_mode() {
  session mode || return
  paddock app share "$app" mode "$work/b" rw --json >"$tmp/mode.json" 2>&1 || { fail mode "$(cat "$tmp/mode.json")"; return; }
  sleep 1
  guest touch-rw "touch \"$shared/ws-2/y\""
  [ -e "$work/b/y" ] || { fail mode "after mode rw the touch made no file: $(cat "$work/a/touch-rw")"; return; }
  pass mode
}

check_handles() {
  session handles || return
  local sock
  sock=$(control)
  local argv
  argv=$(python3 -c 'import json,sys; print(json.dumps({"op":"bench.spawn_child","argv":["/bin/sh","-c",sys.argv[1]]}))' \
    "exec 3<\"$shared/ws-2/in-b.txt\"; sleep 60")
  printf 'bench op %s\n' "$argv" | nc -U "$sock" >/dev/null 2>&1
  local n=0 i=0
  while [ "$i" -lt 15 ]; do
    paddock app share "$app" list --json >"$tmp/list.json" 2>&1
    n=$(json '[r.get("open_handles",0) for r in d["folders"] if r["path"].endswith("/b")][0]' <"$tmp/list.json" 2>/dev/null || echo 0)
    [ "${n:-0}" -ge 1 ] && break
    sleep 1; i=$((i + 1))
  done
  [ "${n:-0}" -ge 1 ] || { fail handles "no open handle counted in 15 s: $(cat "$tmp/list.json")"; return; }
  paddock app share "$app" mode "$work/b" ro --json >"$tmp/reopen.json" 2>&1
  [ "$(json 'd["live"]["reopen_needed"]' <"$tmp/reopen.json")" = True ] || { fail handles "no reopen_needed: $(cat "$tmp/reopen.json")"; return; }
  paddock app share "$app" list --json >"$tmp/list2.json"
  grep -q '"status": "reopen-files"' "$tmp/list2.json" || { fail handles "no reopen-files row"; return; }
  if paddock app share "$app" remove "$work/b" >"$tmp/rm.out" 2>&1; then
    fail handles "remove with open files passed without --force"; return
  fi
  grep -q "files open in" "$tmp/rm.out" || { fail handles "the refusal does not say why: $(cat "$tmp/rm.out")"; return; }
  pass handles
}

check_remove() {
  session remove || return
  paddock app share "$app" remove "$work/b" --force --json >"$tmp/remove.json" 2>&1 || { fail remove "$(cat "$tmp/remove.json")"; return; }
  sleep 1
  guest ls-gone "ls \"$shared/ws-2\""
  grep -q in-b.txt "$work/a/ls-gone" && { fail remove "the guest still lists ws-2"; return; }
  pass remove
}

check_refuse() {
  session refuse || return
  local before
  before=$(shasum "$profile/grants.json" 2>/dev/null)
  for p in "$HOME" "$HOME/Library" "$XDG_DATA_HOME/paddock"; do
    if paddock app share "$app" add "$p" >"$tmp/refuse.out" 2>&1; then
      fail refuse "$p was shared"; return
    fi
    [ "$(wc -l <"$tmp/refuse.out")" -le 3 ] || { fail refuse "more than one line for $p: $(cat "$tmp/refuse.out")"; return; }
    grep -q "paddock does not share" "$tmp/refuse.out" || { fail refuse "no reason for $p: $(cat "$tmp/refuse.out")"; return; }
  done
  [ "$(shasum "$profile/grants.json" 2>/dev/null)" = "$before" ] || { fail refuse "grants.json changed"; return; }
  pass refuse
}

check_network() {
  session network || return
  paddock app network "$app" none --json >"$tmp/net.json" 2>&1 || { fail network "$(cat "$tmp/net.json")"; return; }
  [ "$(json 'd["change"]["applies"]' <"$tmp/net.json")" = live ] || { fail network "not live"; return; }
  guest curl "/usr/bin/curl -sS -m 10 -o /dev/null https://example.com"
  grep -q "exit 0" "$work/a/curl" && { fail network "curl passed in mode none"; return; }
  paddock app status "$app" --json >"$tmp/status.json"
  [ "$(json 'd["network"]["mode"]' <"$tmp/status.json")" = none ] || { fail network "status says another mode"; return; }
  paddock app network "$app" allowlist >/dev/null 2>&1
  pass network
}

check_clipboard() {
  session clipboard || return
  paddock app clipboard "$app" --to-app off --json >"$tmp/clip.json" 2>&1 || { fail clipboard "$(cat "$tmp/clip.json")"; return; }
  [ "$(json 'd["change"]["applies"]' <"$tmp/clip.json")" = live ] || { fail clipboard "not live"; return; }
  paddock app status "$app" --json >"$tmp/status.json"
  [ "$(json 'd["clipboard"]["to_app"]' <"$tmp/status.json")" = False ] || { fail clipboard "status says to_app on"; return; }
  if [ "${PADDOCK_GRANTS_LOOK:-}" = 1 ]; then
    echo "check-me" | pbcopy
    echo "look: click into the TextEdit window of the session and press Command-V. Did nothing paste? [y/n]"
    read -r a
    [ "$a" = y ] || { fail clipboard "the paste reached the app"; return; }
  fi
  paddock app clipboard "$app" --to-app on >/dev/null 2>&1
  pass clipboard
}

check_status() {
  session status || return
  paddock app status "$app" --json >"$tmp/status.json"
  for k in open_handles_per_share clipboard devices changes; do
    grep -q "\"$k\"" "$tmp/status.json" || { fail status "no $k"; return; }
  done
  [ "$(json 'all(x["applies"] == "next-start" for x in d["devices"])' <"$tmp/status.json")" = True ] || { fail status "a device is not next-start"; return; }
  [ "$(json 'len(d["changes"]) >= 6' <"$tmp/status.json")" = True ] || { fail status "fewer than six changes"; return; }
  [ "$(json 'd["changes"][0]["t"] >= d["changes"][-1]["t"]' <"$tmp/status.json")" = True ] || { fail status "not newest first"; return; }
  [ "$(grep -c '"kind":"grant"' "$events")" -ge 6 ] || { fail status "fewer than six grant lines in events.jsonl"; return; }
  grep '"kind":"grant"' "$events" | head -1 | grep -q '"grant":{' || { fail status "a grant line has no grant object"; return; }
  pass status
}

check_persist() {
  session persist || return
  paddock app share "$app" add "$work/c" >/dev/null 2>&1 || { fail persist "add of c failed"; return; }
  paddock app stop "$app" --wait 60 >/dev/null 2>&1
  sleep 2
  start_session || { fail persist "the second start failed: $(tail -3 "$tmp/run.out" | tr '\n' ' ')"; return; }
  sleep 3
  paddock app share "$app" list --json >"$tmp/list3.json"
  [ "$(json '[r["status"] for r in d["folders"] if r["path"].endswith("/c")][0]' <"$tmp/list3.json")" = live ] || { fail persist "c is not live after the start: $(cat "$tmp/list3.json")"; return; }
  pass persist
}

run() {
  case "$1" in
    build) build ;;
    add) check_add ;;
    mode) check_mode ;;
    handles) check_handles ;;
    remove) check_remove ;;
    refuse) check_refuse ;;
    network) check_network ;;
    clipboard) check_clipboard ;;
    status) check_status ;;
    persist) check_persist ;;
    *) echo "unknown check $1"; exit 2 ;;
  esac
}

if [ "$what" = all ]; then
  for c in build add mode handles remove refuse network clipboard status persist; do run "$c"; done
else
  build
  # A single check runs the checks before it that make its state.
  case "$what" in
    mode) run add ;;
    handles) run add ;;
    remove) run add ;;
  esac
  run "$what"
fi
exit $((fails > 0))
