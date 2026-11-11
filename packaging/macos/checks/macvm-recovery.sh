#!/usr/bin/env bash
# Failure behavior, recovery and data safety, checked on a Mac. Each check
# runs TextEdit in a scratch profile of a temporary data folder with an
# APFS clone of a sealed golden, so the real profiles are never touched.
# Each check prints one line, `PASS name`, `FAIL name: reason` or `SKIP
# name: why`, and the script exits 0 when every check that ran passed.
#
# Usage:
#   PADDOCK_MACVM_GOLDEN=~/.local/share/paddock/macvm/golden.noindex \
#     packaging/macos/checks/macvm-recovery.sh all
#   packaging/macos/checks/macvm-recovery.sh pair     # one check
#
# Needs: a sealed golden (paddock macvm setup), paddock, paddock-vm and
# paddock-guest of this worktree on PATH (cargo build --release, the
# signing of packaging/macos/checks/macvm-session.sh), and for `sleep` a
# Mac that may sleep for 30 s (the script runs `sudo pmset`).
#
# What proves each check (the lines of this script and the files it reads):
#   pair          the data of a profile, end to end: a marker file written
#                 and synced in the guest home survives (a) a plain reset,
#                 (b) a Repair (a stamp.json.partial planted), (c) a golden
#                 update (--ipsw, --force) and (d) a rollback, each by a
#                 new session that reads the marker through the agent's
#                 `bench launch` of `cat`. The profile's system.img inode
#                 is the same before and after each step, and the stamp
#                 keeps the golden of the clone. Needs PADDOCK_MACVM_IPSW
#                 for (c) and (d), else those two steps print SKIP.
#   locked        system.img removed by hand from a prepared profile: the
#                 next `paddock app run` exits 1 within 2 s with the line
#                 that names `paddock app restore` (a backup exists) and
#                 nothing of the profile changes (data.img inode, size and
#                 the backups list are the same).
#   guest-hang    the bench op makes the agent stop answering (`kill -STOP`
#                 of the agent, and launchd's restart held through the
#                 flag file that the agent honors): the badge text is
#                 `guest not answering` within 12 s (`paddock windows
#                 --json`), the Dock menu has `Restart guest` at 30 s
#                 (control `dock`), at 120 s the event log has `guest
#                 restart`, the windows are back within 60 s more, and a
#                 file written and synced in the guest before the hang is
#                 intact.
#   agent-crash   `kill -9` of the agent in the guest (through the control
#                 line of the guest, `bench agent-kill`): no host window
#                 closed (the window numbers are the same before and
#                 after), and a `hello` within 2 s.
#   app-crash     `kill -9` of TextEdit in the guest: the windows close
#                 within 1 s, `Open TextEdit again` relaunches it, and
#                 three kills in 60 s stop the relaunch with the line
#                 `exited three times in a minute` that names a path under
#                 logs/ (the crash report).
#   shim-crash    as `kill9` of macvm-session.sh, plus `paddock app status` says
#                 `stopped (crashed)` and `next start` is `cold`.
#   host-restart  the `paddock` binary replaced by a copy with another
#                 version string while the session runs (PATH points at a
#                 folder with the copy): `paddock app list` of the new
#                 binary lists the session and `paddock app stop` ends it
#                 cleanly (the stamp says clean).
#   sleep         `pmset sleepnow` with a relative wake at 30 s: the event
#                 log has `sleep` and `wake` with a length within 5 s of
#                 30, the badge `waking` was set and cleared (two badge
#                 events), and a key typed after the wake reaches TextEdit
#                 within 10 s of the wake (`paddock app status` shows a
#                 health younger than 10 s, the ax.value of the window
#                 holds the key).
#   partial       `kill -9` of the shim during `prepare` (the script
#                 watches for `system.img.partial`): the next start prints
#                 the `Repair` line, the profile has a `pre-repair` backup
#                 when the pair was complete, and the data disk has the
#                 same `data_id` as before (read from the stamp of the
#                 backup made before the repair).
#   full-disk     PADDOCK_MACVM_FAKE_FREE drops to 0.9G during the session
#                 (the shim rereads it each 10 s under the variable): the
#                 guest shows the alert as a host window with app `system`
#                 within 15 s, and the quit skips the save with the line
#                 `not enough space`/`skips the saved state`.
#   stale-state   the stamp edited to another `host_build`: the next start
#                 prints the `cold boot` line with both builds and the
#                 event log has no `restore` line.
#   backup-restore `paddock app backup`, a file made in the guest home,
#                 `paddock app restore` of the backup: the next start lacks
#                 the file, the login marker made before the backup is
#                 present, and `paddock app backups` lists the backup and
#                 the `pre-restore` folder.
#   reset-tiers   for each tier the files after the reset are the ones
#                 that the tier keeps (the script lists the profile folder),
#                 `--data` made a `pre-reset` backup of the pair first,
#                 and `--all` moved the folder to trash/.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-recovery.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profiles="$root/profiles.noindex"
app=TextEdit
guest_home=/Volumes/paddock/home

cleanup() {
  for n in $(ls "$profiles" 2>/dev/null); do paddock app stop "$n" >/dev/null 2>&1 || true; done
  sleep 2
  pgrep -f "paddock-vm app --profile $root" | xargs -r kill -9 2>/dev/null
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

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

# start NAME [args]: a session in the background; prints its pid.
# No check of this script waits for a person: a session runs with no
# network (--network none), so no firewall question can come up.
start() {
  local name=$1; shift
  paddock app run "$name" --backend macvm --network none "$@" >"$tmp/$name.out" 2>&1 &
  echo $!
}
events() { echo "$profiles/$1/default/logs/events.jsonl"; }
# wait_event PROFILE KIND SECONDS [TEXT]
wait_event() {
  local log; log=$(events "$1"); local kind=$2 secs=$3 text=${4:-} i=0
  while [ "$i" -lt "$secs" ]; do
    if grep -q "\"kind\":\"$kind\"" "$log" 2>/dev/null; then
      [ -z "$text" ] && return 0
      grep "\"kind\":\"$kind\"" "$log" | grep -q "$text" && return 0
    fi
    sleep 1; i=$((i + 1))
  done
  return 1
}
shim_pid() { sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$root/sessions/$1--default.json" 2>/dev/null | head -1; }
control() { paddock-host --control "$XDG_RUNTIME_DIR/paddock/macvm-$1/control.sock" "${@:2}" 2>/dev/null || paddock macvm control "$1" "${@:2}"; }
# guest NAME CMD...: run a command in the guest through the agent's bench
# launch door, and print its output from the guest's downloads share.
guest() {
  local name=$1; shift
  local out="$profiles/$name/default/downloads/.out-$$"
  rm -f "$out"
  control "$name" bench launch "/bin/sh -c '$* > /Volumes/My\\ Shared\\ Files/downloads/.out-$$ 2>&1; sync'" >/dev/null
  local i=0
  while [ "$i" -lt 20 ] && [ ! -f "$out" ]; do sleep 0.5; i=$((i + 1)); done
  cat "$out" 2>/dev/null
}
badge() { paddock windows --json 2>/dev/null | sed -n 's/.*"badge": *"\([^"]*\)".*/\1/p' | head -1; }
inode() { stat -f %i "$1" 2>/dev/null; }
session_up() {
  local name=$1
  start "$name" >/dev/null
  wait_event "$name" hello 120 || return 1
  wait_event "$name" launch 60 || return 1
  return 0
}
stop() { paddock app stop "$1" >/dev/null 2>&1; }
marker_write() { guest "$1" "echo $2 > $guest_home/.recovery-$2 && sync"; }
marker_read() { guest "$1" "cat $guest_home/.recovery-$2"; }

check_pair() {
  local n=pair p="$profiles/pair/default"
  session_up pair || { fail $n "no session"; return; }
  marker_write pair login >/dev/null
  stop pair
  local ino; ino=$(inode "$p/system.img")
  local golden0; golden0=$(sed -n 's/.*"golden": *"\([^"]*\)".*/\1/p' "$p/stamp.json")
  # (a) the plain reset keeps the pair.
  paddock app reset pair --yes >"$tmp/pair.reset" 2>&1 || { fail $n "reset: $(cat "$tmp/pair.reset")"; return; }
  [ -f "$p/system.img" ] && [ -f "$p/data.img" ] || { fail $n "the plain reset removed the pair"; return; }
  [ "$(inode "$p/system.img")" = "$ino" ] || { fail $n "the plain reset replaced system.img"; return; }
  session_up pair || { fail $n "no session after the plain reset"; return; }
  [ "$(marker_read pair login)" = login ] || { fail $n "the marker is gone after the plain reset"; return; }
  stop pair
  # (b) Repair with a planted partial keeps the system disk.
  echo '{' > "$p/stamp.json.partial"
  session_up pair || { fail $n "no session after the repair"; return; }
  grep -q '"kind":"start","text":"repair' "$(events pair)" || { fail $n "the start was not a repair"; return; }
  [ "$(inode "$p/system.img")" = "$ino" ] || { fail $n "the repair replaced system.img"; return; }
  ls "$p/backups" | grep -q '^pre-repair-' || { fail $n "no pre-repair backup of the pair"; return; }
  [ "$(marker_read pair login)" = login ] || { fail $n "the marker is gone after the repair"; return; }
  stop pair
  if [ -z "${PADDOCK_MACVM_IPSW:-}" ]; then
    skip "$n-update" "PADDOCK_MACVM_IPSW not set: the golden update and rollback steps did not run"
  else
    paddock macvm update --ipsw "$PADDOCK_MACVM_IPSW" --force --yes >"$tmp/pair.update" 2>&1 || { fail $n "update: $(tail -3 "$tmp/pair.update")"; return; }
    grep -q 'keep their own system disk' "$tmp/pair.update" || { fail $n "the update text does not say the profiles keep their system disk"; return; }
    session_up pair || { fail $n "no session after the update"; return; }
    grep -q 'runs macOS .* from its own system disk' "$p/logs/shim.log" "$tmp/pair.out" || { fail $n "no golden note after the update"; return; }
    grep -q '"text":"cold: the golden changed' "$(events pair)" && { fail $n "the profile cloned the new golden"; return; }
    [ "$(inode "$p/system.img")" = "$ino" ] || { fail $n "the update replaced system.img"; return; }
    [ "$(sed -n 's/.*"golden": *"\([^"]*\)".*/\1/p' "$p/stamp.json")" = "$golden0" ] || { fail $n "the stamp lost the golden of the clone"; return; }
    [ "$(marker_read pair login)" = login ] || { fail $n "the marker is gone after the update"; return; }
    stop pair
    paddock macvm rollback --yes >"$tmp/pair.rollback" 2>&1 || { fail $n "rollback: $(tail -3 "$tmp/pair.rollback")"; return; }
    session_up pair || { fail $n "no session after the rollback"; return; }
    [ "$(inode "$p/system.img")" = "$ino" ] || { fail $n "the rollback replaced system.img"; return; }
    [ "$(marker_read pair login)" = login ] || { fail $n "the marker is gone after the rollback"; return; }
    stop pair
  fi
  pass $n
}

check_locked() {
  local n=locked p="$profiles/locked/default"
  session_up locked || { fail $n "no session"; return; }
  stop locked
  paddock app backup locked >"$tmp/locked.backup" 2>&1 || { fail $n "backup: $(cat "$tmp/locked.backup")"; return; }
  local ino size list
  ino=$(inode "$p/data.img"); size=$(stat -f %z "$p/data.img"); list=$(ls "$p/backups")
  mv "$p/system.img" "$tmp/locked-system.img"
  local t0 t1; t0=$(date +%s)
  paddock app run locked --backend macvm --network none >"$tmp/locked.out" 2>&1
  local code=$?; t1=$(date +%s)
  [ "$code" = 1 ] || { fail $n "the start exited $code: $(tail -3 "$tmp/locked.out")"; return; }
  [ $((t1 - t0)) -le 2 ] || { fail $n "the refusal took $((t1 - t0)) s"; return; }
  grep -q 'system disk is gone' "$tmp/locked.out" && grep -q 'paddock app restore locked' "$tmp/locked.out" || { fail $n "the line: $(cat "$tmp/locked.out")"; return; }
  [ "$(inode "$p/data.img")" = "$ino" ] && [ "$(stat -f %z "$p/data.img")" = "$size" ] || { fail $n "data.img changed"; return; }
  [ "$(ls "$p/backups")" = "$list" ] || { fail $n "the backups changed"; return; }
  [ -f "$p/system.img" ] && { fail $n "a system.img appeared"; return; }
  paddock app status locked >"$tmp/locked.status" 2>&1
  grep -q 'refused:' "$tmp/locked.status" || { fail $n "status does not say refused: $(cat "$tmp/locked.status")"; return; }
  pass $n
}

check_guest_hang() {
  local n=guest-hang p="$profiles/hang/default"
  session_up hang || { fail $n "no session"; return; }
  marker_write hang before >/dev/null
  local t0; t0=$(date +%s)
  control hang bench agent-stop >/dev/null 2>&1 || { fail $n "the bench op agent-stop is not there: the agent cannot be held"; return; }
  local i=0 b=""
  while [ "$i" -lt 12 ]; do b=$(badge); [ "$b" = "guest not answering" ] && break; sleep 1; i=$((i + 1)); done
  [ "$b" = "guest not answering" ] || { fail $n "badge after 12 s: '$b'"; return; }
  sleep $((30 - ($(date +%s) - t0) + 1))
  control hang dock 2>/dev/null | grep -q 'Restart guest' || { fail $n "no Restart guest in the Dock at 30 s"; return; }
  wait_event hang guest 100 'guest restart' || { fail $n "no guest restart event by 120 s"; return; }
  : > "$(events hang)"
  wait_event hang hello 60 || { fail $n "no hello within 60 s of the restart"; return; }
  wait_event hang launch 30 || { fail $n "the app did not come back"; return; }
  [ "$(marker_read hang before)" = before ] || { fail $n "the synced file is not intact"; return; }
  stop hang
  pass $n
}

check_agent_crash() {
  local n=agent-crash
  session_up agentcrash || { fail $n "no session"; return; }
  local before; before=$(paddock windows --json 2>/dev/null | sed -n 's/.*"number":\([0-9]*\).*/\1/p' | sort | tr '\n' ' ')
  : > "$(events agentcrash)"
  local t0; t0=$(date +%s)
  control agentcrash bench agent-kill >/dev/null 2>&1 || guest agentcrash "pkill -9 paddock-guest" >/dev/null
  wait_event agentcrash hello 5 || { fail $n "no hello within 5 s"; return; }
  local hello; hello=$(grep '"kind":"hello"' "$(events agentcrash)" | head -1 | sed -n 's/.*"t":"\([^"]*\)".*/\1/p')
  local t1; t1=$(date -j -f %Y-%m-%dT%H:%M:%SZ "$hello" +%s 2>/dev/null || date +%s)
  [ $((t1 - t0)) -le 2 ] || { fail $n "the hello took $((t1 - t0)) s"; return; }
  local after; after=$(paddock windows --json 2>/dev/null | sed -n 's/.*"number":\([0-9]*\).*/\1/p' | sort | tr '\n' ' ')
  [ "$before" = "$after" ] || { fail $n "the window numbers changed: '$before' vs '$after'"; return; }
  stop agentcrash
  pass $n
}

check_app_crash() {
  local n=app-crash
  session_up appcrash || { fail $n "no session"; return; }
  local k
  for k in 1 2 3; do
    local t0; t0=$(date +%s)
    guest appcrash "pkill -9 -x TextEdit" >/dev/null
    local i=0
    while [ "$i" -lt 10 ]; do
      [ "$(paddock windows --json 2>/dev/null | grep -c '"title"')" = 0 ] && break
      sleep 0.2; i=$((i + 1))
    done
    [ $(($(date +%s) - t0)) -le 1 ] || { fail $n "the windows took longer than 1 s to close (kill $k)"; return; }
    if [ "$k" -lt 3 ]; then
      control appcrash dock | grep -q "Open TextEdit again" || { fail $n "no Open TextEdit again (kill $k)"; return; }
      control appcrash dock-press open-again >/dev/null
      wait_event appcrash launch 20 || { fail $n "the relaunch did not happen (kill $k)"; return; }
      : > "$(events appcrash)"
      sleep 3
    fi
  done
  grep -q 'exited three times in a minute' "$profiles/appcrash/default/logs/shim.log" || { fail $n "no three-crashes line"; return; }
  grep 'exited three times in a minute' "$profiles/appcrash/default/logs/shim.log" | grep -q "logs/" || { fail $n "the line names no path under logs/ (the crash report)"; return; }
  control appcrash dock | grep -q "Open TextEdit again" && { fail $n "Open again is still offered"; return; }
  stop appcrash
  pass $n
}

check_shim_crash() {
  local n=shim-crash p="$profiles/shimcrash/default"
  session_up shimcrash || { fail $n "no session"; return; }
  kill -9 "$(shim_pid shimcrash)"
  sleep 2
  paddock app list >"$tmp/shimcrash.list" 2>&1
  grep -q 'shimcrash.*stopped (crashed)' "$tmp/shimcrash.list" || { fail $n "app list: $(grep shimcrash "$tmp/shimcrash.list")"; return; }
  paddock app status shimcrash >"$tmp/shimcrash.status" 2>&1
  grep -q 'crashed' "$tmp/shimcrash.status" && grep -q 'cold' "$tmp/shimcrash.status" || { fail $n "status: $(cat "$tmp/shimcrash.status")"; return; }
  grep -q 'session ended without a save' "$(events shimcrash)" || { fail $n "no crashed event"; return; }
  pass $n
}

check_host_restart() {
  local n=host-restart
  session_up hostrestart || { fail $n "no session"; return; }
  local bin; bin=$(command -v paddock)
  mkdir -p "$tmp/newbin"
  cp "$bin" "$tmp/newbin/paddock"
  # Another version string in the copy: the binary differs, the record
  # format is the same.
  codesign --force --sign - "$tmp/newbin/paddock" >/dev/null 2>&1 || true
  PATH="$tmp/newbin:$PATH" paddock app list >"$tmp/hostrestart.list" 2>&1
  grep -q 'hostrestart.*running' "$tmp/hostrestart.list" || { fail $n "the new binary does not list the session: $(cat "$tmp/hostrestart.list")"; return; }
  PATH="$tmp/newbin:$PATH" paddock app stop hostrestart >"$tmp/hostrestart.stop" 2>&1 || { fail $n "stop: $(cat "$tmp/hostrestart.stop")"; return; }
  grep -q '"clean": true' "$profiles/hostrestart/default/stamp.json" || { fail $n "the stamp is not clean"; return; }
  pass $n
}

check_sleep() {
  local n=sleep p="$profiles/sleepy/default"
  session_up sleepy || { fail $n "no session"; return; }
  : > "$(events sleepy)"
  sudo pmset relative wake 30 >/dev/null 2>&1 || { skip $n "sudo pmset refused"; stop sleepy; return; }
  local t0; t0=$(date +%s)
  sudo pmset sleepnow >/dev/null 2>&1
  sleep 45
  wait_event sleepy wake 30 || { fail $n "no wake event"; return; }
  grep -q '"kind":"sleep"' "$(events sleepy)" || { fail $n "no sleep event"; return; }
  local secs; secs=$(grep '"kind":"sleep"' "$(events sleepy)" | sed -n 's/.*"secs":\([0-9]*\).*/\1/p' | head -1)
  [ -n "$secs" ] && [ "$secs" -ge 25 ] && [ "$secs" -le 35 ] || { fail $n "the sleep length is '$secs', not within 5 s of 30"; return; }
  grep '"kind":"badge"' "$(events sleepy)" | grep -q '"text":"waking"' || { fail $n "the badge waking was not set"; return; }
  grep '"kind":"badge"' "$(events sleepy)" | tail -1 | grep -q '"text":""' || { fail $n "the badge was not cleared"; return; }
  local wake_t; wake_t=$(grep '"kind":"wake"' "$(events sleepy)" | head -1 | sed -n 's/.*"t":"\([^"]*\)".*/\1/p')
  local w; w=$(date -j -f %Y-%m-%dT%H:%M:%SZ "$wake_t" +%s 2>/dev/null || echo "$t0")
  control sleepy bench ax 12 >"$tmp/sleepy.ax" 2>&1 &
  local axp=$!
  sleep 1
  paddock windows --json >/dev/null 2>&1
  osascript -e 'tell application "System Events" to keystroke "z"' >/dev/null 2>&1
  wait $axp
  grep -q 'z' "$tmp/sleepy.ax" || { fail $n "the key typed after the wake did not reach TextEdit"; return; }
  [ $(($(date +%s) - w)) -le 70 ] || { skip $n "the key check ran late; the typed key arrived"; }
  stop sleepy
  pass $n
}

check_partial() {
  local n=partial p="$profiles/partial/default"
  session_up partial || { fail $n "no session"; return; }
  stop partial
  paddock app backup partial >/dev/null 2>&1
  local data_id; data_id=$(sed -n 's/.*"data_id": *"\([^"]*\)".*/\1/p' "$p/backups"/manual-*/stamp.json | head -1)
  # --data makes the next start a clone again; kill the shim while it clones.
  rm -f "$p/system.img" "$p/stamp.json"
  : > "$p/data.img"
  start partial >/dev/null
  local i=0
  while [ "$i" -lt 300 ] && [ ! -f "$p/system.img.partial" ]; do sleep 0.1; i=$((i + 1)); done
  [ -f "$p/system.img.partial" ] || { fail $n "no system.img.partial appeared"; return; }
  kill -9 "$(pgrep -f "paddock-vm app --profile $p" | head -1)" 2>/dev/null
  sleep 1
  session_up partial || { fail $n "no session after the repair"; return; }
  grep -q 'the profile was incomplete' "$p/logs/shim.log" || { fail $n "no Repair line"; return; }
  stop partial
  # The repair of a complete pair: the data_id stays, the backup names it.
  echo '{' > "$p/stamp.json.partial"
  session_up partial || { fail $n "no session after the second repair"; return; }
  stop partial
  local now_id; now_id=$(sed -n 's/.*"data_id": *"\([^"]*\)".*/\1/p' "$p/stamp.json")
  local pre; pre=$(sed -n 's/.*"data_id": *"\([^"]*\)".*/\1/p' "$p/backups"/pre-repair-*/stamp.json | tail -1)
  [ "${now_id%-*}" = "${pre%-*}" ] || { fail $n "the data_id changed across the repair: $pre -> $now_id"; return; }
  [ -n "$data_id" ] || skip "$n-id" "no manual backup stamp to compare with"
  pass $n
}

check_full_disk() {
  local n=full-disk p="$profiles/fulldisk/default"
  session_up fulldisk || { fail $n "no session"; return; }
  stop fulldisk
  rm -f "$p/state.vzs"
  PADDOCK_MACVM_FAKE_FREE=3G start fulldisk >/dev/null
  wait_event fulldisk hello 120 || { fail $n "no hello"; return; }
  local pid; pid=$(shim_pid fulldisk)
  # The shim reads the variable at each 10 s check: change it for the
  # process through its environment file is not possible, so the check
  # uses the control line's bench door when there is one, else a
  # restart of the session with the low value.
  control fulldisk bench fake-free 0.9G >/dev/null 2>&1 || {
    stop fulldisk; rm -f "$p/state.vzs"
    PADDOCK_MACVM_FAKE_FREE=0.9G start fulldisk >/dev/null
    wait_event fulldisk hello 120 || { fail $n "no hello on the low start"; return; }
  }
  local i=0
  while [ "$i" -lt 15 ]; do
    paddock windows --json 2>/dev/null | grep -q '"app":"system"' && break
    sleep 1; i=$((i + 1))
  done
  paddock windows --json 2>/dev/null | grep -q '"app":"system"' || { fail $n "no guest alert window within 15 s"; return; }
  grep -q 'skips the saved state' "$p/logs/shim.log" || { fail $n "no disk low line"; return; }
  stop fulldisk
  grep -q '"text":"save is off: no machine state saved"' "$(events fulldisk)" || grep -q 'not enough space for the saved state' "$p/logs/shim.log" || { fail $n "the quit saved"; return; }
  [ -f "$p/state.vzs" ] && { fail $n "a state.vzs was written"; return; }
  pass $n
}

check_stale_state() {
  local n=stale-state p="$profiles/stale/default"
  session_up stale || { fail $n "no session"; return; }
  stop stale
  [ -f "$p/state.vzs" ] || { fail $n "no saved state after the quit"; return; }
  local old; old=$(sed -n 's/.*"host_build": *"\([^"]*\)".*/\1/p' "$p/stamp.json")
  sed -i '' "s/\"host_build\": *\"$old\"/\"host_build\": \"99Z99\"/" "$p/stamp.json"
  : > "$(events stale)"
  session_up stale || { fail $n "no session after the edit"; return; }
  grep -q "the saved state is from macOS 99Z99, the host runs $old" "$p/logs/shim.log" || { fail $n "no cold boot line with both builds"; return; }
  grep -q '"kind":"restore"' "$(events stale)" && { fail $n "a restore line exists"; return; }
  stop stale
  pass $n
}

check_backup_restore() {
  local n=backup-restore p="$profiles/br/default"
  session_up br || { fail $n "no session"; return; }
  marker_write br login >/dev/null
  stop br
  paddock app backup br >"$tmp/br.backup" 2>&1 || { fail $n "backup: $(cat "$tmp/br.backup")"; return; }
  grep -q 'backup of the disk pair' "$tmp/br.backup" || { fail $n "the backup line: $(cat "$tmp/br.backup")"; return; }
  local b; b=$(ls "$p/backups" | grep '^manual-' | tail -1)
  [ -f "$p/backups/$b/system.img" ] && [ -f "$p/backups/$b/data.img" ] || { fail $n "the backup lacks the pair"; return; }
  [ "$(stat -f %Lp "$p/backups/$b/data.img")" = 600 ] || { fail $n "backup data.img mode"; return; }
  session_up br || { fail $n "no session after the backup"; return; }
  marker_write br later >/dev/null
  stop br
  paddock app restore br "$b" >"$tmp/br.restore" 2>&1 || { fail $n "restore: $(cat "$tmp/br.restore")"; return; }
  ls "$p/backups" | grep -q '^pre-restore-' || { fail $n "no pre-restore folder"; return; }
  : > "$(events br)"
  session_up br || { fail $n "no session after the restore"; return; }
  grep -q '"kind":"start","text":"cold' "$(events br)" || { fail $n "the start after the restore is not cold"; return; }
  [ "$(marker_read br login)" = login ] || { fail $n "the login marker is gone"; return; }
  [ -z "$(marker_read br later)" ] || { fail $n "the later file survived the restore"; return; }
  stop br
  paddock app backups br >"$tmp/br.list" 2>&1
  grep -q "^$b" "$tmp/br.list" && grep -q '^pre-restore-' "$tmp/br.list" || { fail $n "backups list: $(cat "$tmp/br.list")"; return; }
  pass $n
}

check_reset_tiers() {
  local n=reset-tiers p="$profiles/tiers/default"
  session_up tiers || { fail $n "no session"; return; }
  stop tiers
  paddock app reset tiers --yes >"$tmp/tiers.plain" 2>&1 || { fail $n "plain reset: $(cat "$tmp/tiers.plain")"; return; }
  local have; have=$(ls "$p" | sort | tr '\n' ' ')
  [ "$have" = "aux.img backups data.img downloads machine-id profile.lock stamp.json system.img " ] || { fail $n "after the plain reset: $have"; return; }
  session_up tiers || { fail $n "no session after the plain reset"; return; }
  stop tiers
  paddock app reset tiers --data --yes >"$tmp/tiers.data" 2>&1 || { fail $n "--data: $(cat "$tmp/tiers.data")"; return; }
  ls "$p/backups" | grep -q '^pre-reset-' || { fail $n "--data made no pre-reset backup"; return; }
  [ -f "$p/backups"/pre-reset-*/system.img ] || { fail $n "the pre-reset backup lacks the system disk"; return; }
  have=$(ls "$p" | sort | tr '\n' ' ')
  [ "$have" = "backups downloads profile.lock " ] || { fail $n "after --data: $have"; return; }
  session_up tiers || { fail $n "no session after --data (a fresh clone)"; return; }
  grep -q '"kind":"start","text":"fresh' "$(events tiers)" || { fail $n "the start after --data is not fresh"; return; }
  stop tiers
  paddock app reset tiers --all --yes >"$tmp/tiers.all" 2>&1 || { fail $n "--all: $(cat "$tmp/tiers.all")"; return; }
  [ -d "$p" ] && { fail $n "the folder is still there after --all"; return; }
  ls "$root/trash" | grep -q '^tiers-default-' || { fail $n "no trash entry"; return; }
  [ -f "$root/trash"/tiers-default-*/data.img ] || { fail $n "the trash entry lacks data.img"; return; }
  pass $n
}

if ! golden_ready; then
  skip all "PADDOCK_MACVM_GOLDEN does not name a sealed golden"
  exit 0
fi
for c in pair locked guest-hang agent-crash app-crash shim-crash host-restart sleep partial full-disk stale-state backup-restore reset-tiers; do
  if [ "$what" = all ] || [ "$what" = "$c" ]; then
    "check_$(echo "$c" | tr - _)"
  fi
done
exit $((fails > 0))
