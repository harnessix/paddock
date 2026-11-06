#!/usr/bin/env bash
# The session helper `paddock-vm app`, checked on a Mac.
#
# Run this script on an Apple silicon Mac. It builds the
# worktree, signs `paddock-vm` with its entitlement, and runs every check
# in a temporary data folder, so it never touches the real paddock data,
# the real profiles, the Keychain or the logins of the Mac. Each check
# prints one line: `PASS name` or `FAIL name: reason`. The script exits 0
# when every check that ran passed.
#
# Usage:
#   packaging/macos/checks/macvm-session.sh build      # compile, clippy, tests
#   packaging/macos/checks/macvm-session.sh all        # build plus every session check
#   packaging/macos/checks/macvm-session.sh fresh      # one check by name
#
# The session checks need a sealed golden image (`paddock macvm setup`)
# and the guest agent in it. Name the golden folder with
# PADDOCK_MACVM_GOLDEN=/path/to/macvm/golden.noindex; the script clones it
# (APFS, no space) into the temporary data folder. Without it the session
# checks print `SKIP` and only `build` runs.
#
# A session reads the guest password from the keyring of `paddock macvm
# setup`: the data disk prepare and the shutdown at the quit need it. The
# script copies the one file of it into the temporary state folder, from
# PADDOCK_MACVM_KEYRING (a folder) or from
# ${XDG_STATE_HOME:-~/.local/state}/paddock/keyring/macvm. Without it the
# session checks print `SKIP`.
#
# PADDOCK_CHECKS="fresh warm" runs a subset with `all`, in that order.
# PADDOCK_CHECK_KEEP=1 keeps the temporary folder (the logs of each
# profile) and prints its path.
#
# What proves each check (the lines of this script and the files it reads):
#   build    `cargo build`, `cargo clippy -D warnings` and
#            `cargo test macvm vm::helper` exit 0 on macOS.
#   fresh    `hello` in events.jsonl within 60 s, no `.partial` in the
#            profile, data.img mode 0600, the profile folder mode 0700.
#   warm     after the data disk is prepared (a quit before that saves
#            nothing, by design), `paddock app stop` ends the session with
#            `quit with save` in
#            events.jsonl, the next start says `warm` and gets `hello`
#            within 10 s.
#   kill9    `kill -9` of the shim: no paddock-vm process after 2 s,
#            `paddock app list` says `stopped (crashed)`, the next start
#            prints `the last session ended without a save: cold boot`.
#            (The fsync file check needs the guest command door of the
#            agent; noted as TODO.)
#   slots    with `[macvm] auto_suspend = false`: two scratch sessions
#            run, a third start exits 2 within 1 s with both holders
#            named, and no third profile folder appears.
#   suspend  automatic suspend (the default): sus-a and sus-b run with
#            their data disks prepared; sus-a started first, so it is the
#            least recently used. A start of sus-c makes sus-a's record
#            say `"suspended": true` within 15 s, `paddock app list` says
#            `suspended`, its events.jsonl has `suspend` lines with the
#            reason `sus-c started`, its shim pid lives on (the Dock face
#            stays), and sus-c gets `hello`. Then `paddock app run sus-a`
#            suspends sus-b (the least recently used now) and sus-a gets a
#            `warm` start and `hello` within 10 s, with a `resume` event
#            that names `paddock app run`. Then `open` of sus-b's Dock
#            face bundle (the reopen of a click on the icon) resumes sus-b
#            warm within 20 s, with a `resume` event that names the Dock
#            icon. INFO prints the seconds of each suspend.
#   instances `paddock app run inst` and `paddock app run inst --instance
#            2` run at once: both get `hello`, the folders default/ and
#            instance-2/ each have their own data.img (other inode) and
#            machine-id (other bytes), the stamps have other data ids,
#            `paddock app list` lists `inst` and `inst 2` as running, and
#            the Dock face shims/inst--2.app says `inst 2 (paddock)`.
#            `paddock app stop inst --instance 2` ends only instance 2.
#   gcdata   after a session of gc-a with a save: `paddock du` has the
#            `macOS guest (macvm)` section with `profile gc-a [stopped]`,
#            `paddock du --json` has `macvm.profiles`, `paddock gc --yes
#            --older-than 0` keeps data.img and system.img of gc-a (only
#            state.vzs and app archives may go), and `paddock gc
#            --older-than 0` lists the profile with `--homes
#            --include-app-logins`.
#   hup      the session started in a `script` pseudo terminal lives 5 s
#            after the terminal closes; `paddock app stop` ends it with
#            `"clean": true` in stamp.json.
#   twice    a second `paddock app run` of the same profile exits 2 with
#            `already runs (pid N)` where N is the first shim's pid.
#   nospace  with PADDOCK_MACVM_FAKE_FREE=2G at quit, the save is skipped
#            with the `not enough space for the saved state` line, the
#            guest shuts down with the password (`quit without save`, no
#            `stopped without save`), the stamp says `"clean": true` and
#            no state.vzs exists.
#   console  logs/console.log has the start line of the agent
#            (`paddock-guest ... starts`) and its `ProductVersion` line
#            after a cold start. (The shim rotates the log at 1 MiB
#            before each boot; the Linux tests prove the rotation.)
#   format   as a plain user, `hdiutil attach -nomount` of a sparse raw
#            file, `diskutil partitionDisk ... GPT APFS Data 100%` and
#            `hdiutil detach` exit 0, and the file starts with a GPT. This
#            is the measurement that the host format needs no root.
#   datadisk a new profile: data.img has a GPT before the first boot
#            (`EFI PART` at byte 512), events.jsonl has `the data disk is
#            prepared` within 120 s and no `dialog` line, shim.log has no
#            `not formatted on the host`, stamp.json says
#            `"data_prepared": true`, and `paddock app status` has the row
#            `data disk  prepared`, events.jsonl has a `copying the home:`
#            progress line, and the script prints `INFO datadisk:` with the
#            size of the home, its time limit, the end line of the copy
#            (unreadable files, guarded folders left empty) and the five
#            slowest parts from agent.log. A second start sends no second
#            prepare (one `preparing the data disk` line in events.jsonl). After the stop the script prints
#            `INFO datadisk:` with the FileVault and encryption lines of
#            `diskutil apfs list` for the attached data.img: the evidence
#            whether a host-made volume is crypto-bound (expected: not
#            encrypted).
#   dialog   an old profile whose data.img is still blank: the script
#            makes the profile folder and an all-zero data.img (no GPT, as
#            before the host formatted new disks) before the first start.
#            The agent partitions it (`the data disk is prepared`, a GPT
#            in data.img after the stop). When the guest shows the disk
#            dialog, events.jsonl has a `dialog` line with `dismissed`.
#            When no dialog line came and agent.log has no `a disk dialog
#            of` line, the guest showed no dialog for the blank disk: the
#            check prints `SKIP dialog:` with the macOS version of the
#            guest, not FAIL.
#   locksettings  a new profile: after its data disk is prepared, the
#            agent log has `machine step NAME: proven true` for each of the
#            eight lock settings (sleep, screen saver of this machine and of
#            every host, its password and delay, the screen lock, the login
#            window's screen saver, the automatic log out). Seconds, not the
#            20 minutes of an idle guest.
#   noapp    `paddock app stop` of a session whose app does not run in
#            the guest returns within 40 s (not 70 s), events.jsonl has
#            `does not run in the guest` and `quit with save`, and
#            state.vzs exists.
#   zombies  after all checks (with `all`), no zombie process of this Mac
#            has a paddock parent (paddock, paddock-vm, the marker); the
#            FAIL line names each parent, and an INFO line gives the counts
#            at the start and at the end.
#   input    after a session, logs/agent.log has no `cannot select the
#            input source` line and at most one `is not available in the
#            guest` line per source. With a Pinyin source as the host's
#            current one, the line names `com.apple.keylayout.ABC`.
#   lock     a `--bench` session after the data disk is prepared: the
#            control line `bench lock` locks the guest screen
#            (SACLockScreenImmediate in the guest). events.jsonl gets
#            `the guest screen locked at` within 10 s, then within 15 s
#            `the guest screen is unlocked` and the `health` of the
#            session record says `"locked": false`: the session typed the
#            guest password. Then `bench launch` of TextEdit gives a
#            window in `window list` within 20 s. The INFO line has the
#            `screen` events (the time, the idle time, the uptime, the
#            tries). The password is in no log: shim.log and events.jsonl
#            must not contain it.
#   keychain a `--bench` session: after the data disk is prepared,
#            events.jsonl has a `keychain` event of the unlock before the
#            app, and the control line `bench keychain` gives a
#            `keychain` event `bench: status unlocked` for the login
#            keychain in /Volumes/paddock/home, with the default keychain
#            and the search list there and the output of `security
#            show-keychain-info`. The same after a stop and a warm start.
#
# The windows of a session (TextEdit through `bench launch`, no app copy,
# no network). The lines of the control socket: `window host-move ID X Y`
# moves the host window as a host drag does (then the session sends
# `window.move`), `window frames ID` gives the host and guest frames, the
# clip radius, the measured radius and the full screen states as JSON,
# `window menu` and `window menu-press PATH` read and press the menu bar.
#   drag     20 host moves of a TextEdit window: each gets the event line
#            `the guest followed the host move` within 2 s, the 95th
#            percentile of their `ms` is below 50, and in 5 samples after
#            each move no `window frames` has host and guest more than
#            2 points apart. INFO prints the p50 and p95.
#   radius   the TextEdit window and an Activity Monitor window (a toolbar,
#            its `title_bar` at least 40) each get a `measured` radius
#            within 5 s, and the clip `radius` is within 1 point of it.
#            No measured radius means no Screen Recording row for the
#            agent in the guest (an old golden: run `paddock macvm setup`).
#   fullscreen  `View > Enter Full Screen` of TextEdit through the menu
#            bar: within 5 s `window frames` says `guest_full_screen` and
#            `full_screen`, the clip radius is 0 and the host frame is the
#            guest display at origin zero; `Exit Full Screen` brings both
#            back, with host and guest within 2 points.
#   login    a `--bench` session with `[host] open-url = "allow"` and
#            PADDOCK_MACVM_LINK_DRY=1 (an approved link opens no host
#            browser, so no tab opens and no vendor is reached). A
#            `bench op` child starts a one-shot HTTP server on the guest
#            loopback port P (`nc -l 127.0.0.1 P` behind a fifo, which
#            answers with the request line it got), a second child runs
#            `/usr/bin/open` of a login link
#            http://127.0.0.1:P/start?redirect_uri=http://127.0.0.1:P/callback
#            in the guest, which reaches the host only through the link
#            helper, the default browser of the guest. events.jsonl gets
#            the `link` line `took the link` and the `login` line `the
#            login callback port P listens on` within 30 s; `lsof` shows
#            P listening on 127.0.0.1 and [::1] only. `curl
#            http://127.0.0.1:P/callback?code=x` on the host then gets
#            `got GET /callback?code=x HTTP/1.1` from the guest server.
#            A guest server on port P+1, which no link named, is not
#            reachable from the host, and 15 s after the callback the
#            port P is closed on the host and events.jsonl says
#            `closed after its callback`.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
# The keyring of the real setup, before the XDG folders move.
keyring_src=${PADDOCK_MACVM_KEYRING:-${XDG_STATE_HOME:-$HOME/.local/state}/paddock/keyring/macvm}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-session.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
# PADDOCK_CHECK_BIN: a folder with a signed paddock and paddock-vm; the
# build step is then skipped.
bin="${PADDOCK_CHECK_BIN:-$here/target/release}"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profiles="$root/profiles.noindex"

cleanup() {
  # End every session of the temporary data folder, hard.
  for rec in "$root"/sessions/*.json; do
    [ -f "$rec" ] || continue
    pid=$(sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$rec" | head -1)
    [ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null
  done
  sleep 2
  pgrep -f "paddock-vm app --profile $profiles" | xargs -r kill -9 2>/dev/null
  if [ -n "${PADDOCK_CHECK_KEEP:-}" ]; then
    echo "kept $tmp"
    return
  fi
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

build() {
  if [ -n "${PADDOCK_CHECK_BIN:-}" ]; then
    skip build "PADDOCK_CHECK_BIN=$bin"; return
  fi
  if ! cargo build --release --bin paddock --bin paddock-vm >"$tmp/build.log" 2>&1; then
    fail build "cargo build: $(tail -5 "$tmp/build.log" | tr '\n' ' ')"; return
  fi
  if ! codesign --force --sign - --entitlements packaging/macos/paddock-vm.entitlements "$bin/paddock-vm" >>"$tmp/build.log" 2>&1; then
    fail build "codesign of paddock-vm failed"; return
  fi
  if ! cargo clippy --all-targets -- -D warnings >"$tmp/clippy.log" 2>&1; then
    fail build "clippy: $(grep -m1 -A3 '^error' "$tmp/clippy.log" | tr '\n' ' ')"; return
  fi
  if ! cargo test macvm >"$tmp/test.log" 2>&1 || ! cargo test vm::helper >>"$tmp/test.log" 2>&1; then
    fail build "tests: $(grep -E 'FAILED|panicked' "$tmp/test.log" | head -3 | tr '\n' ' ')"; return
  fi
  pass build
}

golden_ready() {
  if [ -z "${PADDOCK_MACVM_GOLDEN:-}" ] || [ ! -f "$PADDOCK_MACVM_GOLDEN/manifest.toml" ]; then
    not_ready="set PADDOCK_MACVM_GOLDEN to a sealed golden folder"
    return 1
  fi
  local keyring="$XDG_STATE_HOME/paddock/keyring/macvm"
  if [ ! -f "$keyring/guest-password" ]; then
    if [ ! -f "$keyring_src/guest-password" ]; then
      not_ready="no guest password in $keyring_src (set PADDOCK_MACVM_KEYRING to the keyring folder of the setup)"
      return 1
    fi
    mkdir -p "$keyring" && chmod 700 "$XDG_STATE_HOME/paddock" "$XDG_STATE_HOME/paddock/keyring" "$keyring"
    cp "$keyring_src/guest-password" "$keyring/guest-password" && chmod 600 "$keyring/guest-password" || return 1
  fi
  if [ ! -d "$root/golden.noindex" ]; then
    mkdir -p "$root"
    # An APFS clone of the golden: no space, seconds.
    cp -c -R "$PADDOCK_MACVM_GOLDEN" "$root/golden.noindex" || return 1
    chmod -R u+w "$root/golden.noindex" 2>/dev/null
  fi
  return 0
}

# start NAME [args]: start a session in the background, print its pid.
# No check of this script waits for a person: a session runs with no
# network (--network none), so no firewall question can come up.
start() {
  local name=$1; shift
  paddock app run "$name" --backend macvm --network none "$@" >"$tmp/$name.out" 2>&1 &
  echo $!
}

# wait_event PROFILE KIND SECONDS: true when events.jsonl of the profile
# gets a line of that kind within the seconds.
wait_event() {
  local log="$profiles/$1/default/logs/events.jsonl" kind=$2 secs=$3 i=0
  while [ "$i" -lt "$secs" ]; do
    grep -q "\"kind\":\"$kind\"" "$log" 2>/dev/null && return 0
    sleep 1; i=$((i + 1))
  done
  return 1
}

# wait_prepared PROFILE SECONDS: true when the data disk of a new profile
# is prepared within the seconds. A quit before that saves nothing, by
# design: the next start prepares the disk again.
wait_prepared() {
  local log="$profiles/$1/default/logs/events.jsonl" i=0
  while [ "$i" -lt "$2" ]; do
    grep -q 'the data disk is prepared' "$log" 2>/dev/null && return 0
    sleep 1; i=$((i + 1))
  done
  return 1
}

# count_event PROFILE KIND: how many lines of that kind events.jsonl has.
count_event() {
  grep -c "\"kind\":\"$2\"" "$profiles/$1/default/logs/events.jsonl" 2>/dev/null || echo 0
}

# wait_new_event PROFILE KIND BEFORE SECONDS: true when the profile gets a
# line of that kind beyond the BEFORE it had, within the seconds.
wait_new_event() {
  local i=0
  while [ "$i" -lt "$4" ]; do
    [ "$(count_event "$1" "$2")" -gt "$3" ] && return 0
    sleep 1; i=$((i + 1))
  done
  return 1
}

# stop_sessions: end every session of the temporary data folder, so a
# check that failed half way leaves no VM for the next one.
stop_sessions() {
  for d in "$profiles"/*; do
    [ -d "$d" ] || continue
    paddock app stop "$(basename "$d")" >/dev/null 2>&1
  done
  sleep 1
  pkill -9 -f "paddock-vm app --profile $profiles" 2>/dev/null
  sleep 1
}

shim_pid() {
  sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$root/sessions/$1--default.json" 2>/dev/null | head -1
}

# copy_info PROFILE: the evidence of the copy of the home in the guest:
# the size and the time limit, the end line, and the slowest parts (from
# logs/agent.log, which the quit fetched).
copy_info() {
  local p="$profiles/$1/default"
  grep -oE '"text":"(the home holds|du could not|copied the home)[^"]*' "$p/logs/events.jsonl" 2>/dev/null | sed "s/^\"text\":\"/INFO $1: /"
  grep 'copy part ' "$p/logs/agent.log" 2>/dev/null | sed -n 's/.*copy part \(.*\): exit \(.*\), \([0-9]*\) ms, timed out \([a-z]*\), \([0-9]*\) not readable.*/\3 ms \1 (exit \2, timed out \4, \5 not readable)/p' \
    | sort -rn | head -5 | sed "s/^/INFO $1 slowest part: /"
}

# zombies: one line per zombie process of this Mac: its pid, the pid and
# command of its parent.
zombies() {
  ps -axo stat=,pid=,ppid= | awk '$1 ~ /^Z/ {print $2, $3}' | while read -r z parent; do
    echo "$z $parent $(ps -o comm= -p "$parent" 2>/dev/null)"
  done
}

# check_zombies: after all checks no zombie may have a paddock parent
# (paddock, paddock-vm, the marker). The line names each parent.
check_zombies() {
  local all mine
  all=$(zombies)
  echo "INFO zombies: $(printf '%s' "$all" | grep -c . ) on this Mac at the end, $zombies_before at the start"
  mine=$(printf '%s\n' "$all" | grep -E 'paddock(-vm)?$' || true)
  if [ -n "$mine" ]; then
    fail zombies "zombies of paddock processes (pid, parent pid, parent): $(printf '%s' "$mine" | tr '\n' ';')"; return
  fi
  pass zombies
}

check_fresh() {
  local p="$profiles/fresh/default"
  start fresh >/dev/null
  if ! wait_event fresh hello 60; then
    fail fresh "no hello within 60 s (see $p/logs/)"; return
  fi
  if find "$p" -name '*.partial' | grep -q .; then fail fresh "a .partial remains"; return; fi
  [ "$(stat -f %Lp "$p/data.img")" = 600 ] || { fail fresh "data.img mode $(stat -f %Lp "$p/data.img")"; return; }
  [ "$(stat -f %Lp "$p")" = 700 ] || { fail fresh "profile mode $(stat -f %Lp "$p")"; return; }
  grep -q '"kind":"start","text":"fresh' "$p/logs/events.jsonl" || { fail fresh "the start event is not fresh"; return; }
  paddock app stop fresh >/dev/null 2>&1
  # The LaunchServices line about the shim bundle: noise, counted here so
  # a change shows (see the commit that added this line).
  echo "INFO fresh: $(grep -c 'sandbox_extension_issue_file_to_process' "$p/logs/shim.log" 2>/dev/null || echo 0) sandbox_extension lines in shim.log, shims in $root/shims"
  pass fresh
}

check_warm() {
  local p="$profiles/warm/default"
  start warm >/dev/null
  wait_event warm hello 90 || { fail warm "no hello on the cold start"; return; }
  wait_prepared warm 120 || { fail warm "the data disk was not prepared in 120 s"; return; }
  paddock app stop warm >"$tmp/warm.stop" 2>&1 || { fail warm "stop: $(cat "$tmp/warm.stop")"; return; }
  grep -q '"text":"quit with save"' "$p/logs/events.jsonl" || { fail warm "the first quit did not save"; return; }
  [ -f "$p/state.vzs" ] || { fail warm "no state.vzs after the quit"; return; }
  : >"$p/logs/events.jsonl"
  start warm >/dev/null
  if ! wait_event warm hello 10; then
    fail warm "no hello within 10 s after the restore"; paddock app stop warm >/dev/null 2>&1; return
  fi
  grep -q '"kind":"start","text":"warm' "$p/logs/events.jsonl" || { fail warm "the second start is not warm"; return; }
  paddock app stop warm >/dev/null 2>&1
  pass warm
}

check_kill9() {
  local p="$profiles/kill9/default"
  start kill9 >/dev/null
  wait_event kill9 hello 90 || { fail kill9 "no hello"; return; }
  local pid; pid=$(shim_pid kill9)
  kill -9 "$pid"
  sleep 2
  if pgrep -f "paddock-vm app --profile $p" >/dev/null; then fail kill9 "a VM process lives after kill -9"; return; fi
  paddock app list >"$tmp/kill9.list" 2>&1
  grep -q 'kill9.*stopped (crashed)' "$tmp/kill9.list" || { fail kill9 "app list: $(grep kill9 "$tmp/kill9.list")"; return; }
  local hellos; hellos=$(count_event kill9 hello)
  start kill9 >/dev/null
  wait_new_event kill9 hello "$hellos" 90 || { fail kill9 "no hello on the restart"; return; }
  grep -q 'the last session ended without a save' "$p/logs/shim.log" || { fail kill9 "no cold boot line after the crash"; return; }
  paddock app stop kill9 >/dev/null 2>&1
  echo "TODO kill9: the fsync file check needs the guest command door of the agent"
  pass kill9
}

# no_auto_suspend / auto_suspend_default: the user configuration of the
# temporary folder with or without `[macvm] auto_suspend = false`.
no_auto_suspend() {
  mkdir -p "$XDG_CONFIG_HOME/paddock"
  printf '[macvm]\nauto_suspend = false\n' >"$XDG_CONFIG_HOME/paddock/config.toml"
}
auto_suspend_default() { rm -f "$XDG_CONFIG_HOME/paddock/config.toml"; }

check_slots() {
  no_auto_suspend
  start slot-a >/dev/null; start slot-b >/dev/null
  wait_event slot-a hello 90 && wait_event slot-b hello 90 || { fail slots "the two sessions did not come up"; return; }
  local t0 t1
  t0=$(date +%s)
  paddock app run slot-c --backend macvm --network none >"$tmp/slot-c.out" 2>&1
  local code=$?
  t1=$(date +%s)
  [ "$code" = 2 ] || { fail slots "the third start exited $code: $(tail -3 "$tmp/slot-c.out")"; return; }
  [ $((t1 - t0)) -le 1 ] || { fail slots "the refusal took $((t1 - t0)) s"; return; }
  grep -q 'two macOS VMs already run on this Mac: slot-[ab] (.*pid [0-9]*) and slot-[ab]' "$tmp/slot-c.out" || { fail slots "the line does not name the holders: $(tail -3 "$tmp/slot-c.out")"; return; }
  [ -d "$profiles/slot-c" ] && { fail slots "a folder for slot-c appeared"; return; }
  paddock app stop slot-a >/dev/null 2>&1; paddock app stop slot-b >/dev/null 2>&1
  auto_suspend_default
  pass slots
}

# suspended PROFILE: true when the record of the session says suspended.
suspended() { grep -q '"suspended": *true' "$root/sessions/$1--default.json" 2>/dev/null; }

# wait_suspended PROFILE SECONDS: true when the record says suspended in time.
wait_suspended() {
  local i=0
  while [ "$i" -lt "$2" ]; do suspended "$1" && return 0; sleep 1; i=$((i + 1)); done
  return 1
}

check_suspend() {
  auto_suspend_default
  local a="$profiles/sus-a/default" b="$profiles/sus-b/default" t0 t1 pid hellos
  start sus-a >/dev/null
  wait_event sus-a hello 90 && wait_prepared sus-a 120 || { fail suspend "sus-a did not come up prepared"; return; }
  start sus-b >/dev/null
  wait_event sus-b hello 90 && wait_prepared sus-b 120 || { fail suspend "sus-b did not come up prepared"; return; }
  pid=$(shim_pid sus-a)
  t0=$(date +%s)
  start sus-c >/dev/null
  wait_suspended sus-a 15 || { fail suspend "sus-a was not suspended within 15 s: $(tail -3 "$tmp/sus-c.out")"; return; }
  t1=$(date +%s)
  echo "INFO suspend: sus-a suspended in $((t1 - t0)) s"
  kill -0 "$pid" 2>/dev/null || { fail suspend "the shim of sus-a ended: its Dock face is gone"; return; }
  grep -q '"kind":"suspend","text":"suspend requested: both VM slots were in use and sus-c started' "$a/logs/events.jsonl" || { fail suspend "no suspend event with its reason: $(grep -m2 '"kind":"suspend"' "$a/logs/events.jsonl")"; return; }
  [ -f "$a/state.vzs" ] || { fail suspend "no state.vzs of sus-a"; return; }
  paddock app list >"$tmp/suspend.list" 2>&1
  grep -q 'sus-a.*suspended' "$tmp/suspend.list" || { fail suspend "app list: $(grep sus-a "$tmp/suspend.list")"; return; }
  paddock app status sus-a --json | grep -q '"state": "suspended"' || { fail suspend "status --json does not say suspended"; return; }
  wait_event sus-c hello 90 || { fail suspend "sus-c got no hello after the suspend"; return; }
  # A resume by paddock app run: sus-b is the least recently used now.
  hellos=$(count_event sus-a hello)
  t0=$(date +%s)
  paddock app run sus-a --backend macvm --network none >"$tmp/sus-a.resume" 2>&1 &
  wait_suspended sus-b 15 || { fail suspend "sus-b was not suspended for the resume of sus-a: $(tail -3 "$tmp/sus-a.resume")"; return; }
  wait_new_event sus-a hello "$hellos" 25 || { fail suspend "sus-a got no hello after the resume"; return; }
  t1=$(date +%s)
  echo "INFO suspend: sus-a resumed in $((t1 - t0)) s, sus-b suspended for it"
  grep -q '"kind":"start","text":"warm' "$a/logs/events.jsonl" || { fail suspend "the resume of sus-a was not warm"; return; }
  grep -q '"kind":"resume","text":"resumed from the saved state after paddock app run' "$a/logs/events.jsonl" || { fail suspend "no resume event of paddock app run"; return; }
  # A resume by a click on the Dock icon: the reopen event of `open`.
  hellos=$(count_event sus-b hello)
  open "$root/shims/sus-b.app" || { fail suspend "open of the Dock face of sus-b failed"; return; }
  wait_new_event sus-b hello "$hellos" 20 || { fail suspend "sus-b did not resume within 20 s of the Dock reopen: $(tail -3 "$b/logs/shim.log")"; return; }
  grep -q '"kind":"resume","text":"resumed from the saved state after a click on its Dock icon' "$b/logs/events.jsonl" || { fail suspend "no resume event of the Dock icon"; return; }
  grep -q '"kind":"start","text":"warm' "$b/logs/events.jsonl" || { fail suspend "the resume of sus-b was not warm"; return; }
  pass suspend
}

check_instances() {
  auto_suspend_default
  local one="$profiles/inst/default" two="$profiles/inst/instance-2"
  start inst >/dev/null
  paddock app run inst --instance 2 --backend macvm --network none >"$tmp/inst2.out" 2>&1 &
  wait_event inst hello 90 || { fail instances "instance 1 got no hello"; return; }
  local i=0
  while [ "$i" -lt 90 ] && ! grep -q '"kind":"hello"' "$two/logs/events.jsonl" 2>/dev/null; do sleep 1; i=$((i + 1)); done
  grep -q '"kind":"hello"' "$two/logs/events.jsonl" 2>/dev/null || { fail instances "instance 2 got no hello: $(tail -3 "$tmp/inst2.out")"; return; }
  [ "$(stat -f %i "$one/data.img")" != "$(stat -f %i "$two/data.img")" ] || { fail instances "both instances have one data.img"; return; }
  cmp -s "$one/machine-id" "$two/machine-id" && { fail instances "both instances have one machine id"; return; }
  local id1 id2
  id1=$(sed -n 's/.*"data_id": *"\([^"]*\)".*/\1/p' "$one/stamp.json" | cut -d- -f1)
  id2=$(sed -n 's/.*"data_id": *"\([^"]*\)".*/\1/p' "$two/stamp.json" | cut -d- -f1)
  [ -n "$id1" ] && [ "$id1" != "$id2" ] || { fail instances "the data ids are not separate: $id1 $id2"; return; }
  paddock app list >"$tmp/inst.list" 2>&1
  grep -q '^inst  .*running' "$tmp/inst.list" && grep -q '^inst 2 .*running' "$tmp/inst.list" || { fail instances "app list: $(grep inst "$tmp/inst.list" | tr '\n' ';')"; return; }
  grep -q '<string>inst 2 (paddock)</string>' "$root/shims/inst--2.app/Contents/Info.plist" 2>/dev/null || { fail instances "the Dock face of instance 2 does not name it"; return; }
  paddock app stop inst --instance 2 >/dev/null 2>&1 || { fail instances "stop of instance 2 failed"; return; }
  grep -q '"text":"quit with save"' "$two/logs/events.jsonl" || { fail instances "instance 2 did not quit with save"; return; }
  [ -n "$(shim_pid inst)" ] && kill -0 "$(shim_pid inst)" 2>/dev/null || { fail instances "the stop of instance 2 ended instance 1"; return; }
  pass instances
}

check_gcdata() {
  local p="$profiles/gc-a/default"
  start gc-a >/dev/null
  wait_event gc-a hello 90 && wait_prepared gc-a 120 || { fail gcdata "gc-a did not come up prepared"; return; }
  paddock app stop gc-a >/dev/null 2>&1
  paddock du >"$tmp/du.txt" 2>&1
  grep -q 'macOS guest (macvm)' "$tmp/du.txt" && grep -q 'profile gc-a \[stopped\]' "$tmp/du.txt" || { fail gcdata "paddock du: $(grep -A3 macvm "$tmp/du.txt" | tr '\n' ';')"; return; }
  paddock du --json | grep -q '"profiles"' || { fail gcdata "paddock du --json has no macvm profiles"; return; }
  paddock gc --yes --older-than 0 >"$tmp/gc.txt" 2>&1
  [ -f "$p/data.img" ] && [ -f "$p/system.img" ] || { fail gcdata "gc --yes removed the disks of the profile: $(cat "$tmp/gc.txt" | tr '\n' ';')"; return; }
  paddock gc --older-than 0 >"$tmp/gc-dry.txt" 2>&1
  grep -q 'macOS guest profile gc-a' "$tmp/gc-dry.txt" && grep -q 'include-app-logins' "$tmp/gc-dry.txt" || { fail gcdata "gc does not list the profile with its flags: $(cat "$tmp/gc-dry.txt" | tr '\n' ';')"; return; }
  echo "INFO gcdata: $(grep -c . "$tmp/gc.txt") lines of gc --yes"
  pass gcdata
}

check_hup() {
  local p="$profiles/hup/default"
  script -q /dev/null bash -c "paddock app run hup --backend macvm --network none" >"$tmp/hup.out" 2>&1 &
  local spid=$!
  wait_event hup hello 90 || { fail hup "no hello"; kill "$spid" 2>/dev/null; return; }
  local pid; pid=$(shim_pid hup)
  kill -HUP "$spid" 2>/dev/null
  sleep 5
  kill -0 "$pid" 2>/dev/null || { fail hup "the shim died with its terminal"; return; }
  paddock app stop hup >/dev/null 2>&1 || { fail hup "stop failed"; return; }
  grep -q '"clean": true' "$p/stamp.json" || { fail hup "the stamp is not clean"; return; }
  pass hup
}

check_twice() {
  start twice >/dev/null
  wait_event twice hello 90 || { fail twice "no hello"; return; }
  local pid; pid=$(shim_pid twice)
  paddock app run twice --backend macvm --network none >"$tmp/twice.out" 2>&1
  local code=$?
  [ "$code" = 2 ] || { fail twice "the second start exited $code"; return; }
  grep -q "already runs (pid $pid)" "$tmp/twice.out" || { fail twice "the line does not name pid $pid: $(cat "$tmp/twice.out")"; return; }
  paddock app stop twice >/dev/null 2>&1
  pass twice
}

check_nospace() {
  local p="$profiles/nospace/default"
  # The first start clones the golden, which needs 8 GiB: no fake value.
  start nospace >/dev/null
  wait_event nospace hello 90 || { fail nospace "no hello"; return; }
  paddock app stop nospace >/dev/null 2>&1
  rm -f "$p/state.vzs"
  # The second start clones nothing, so 2G passes the start check and
  # fails the save check (the RAM size plus 1 GiB) at the quit.
  local hellos; hellos=$(count_event nospace hello)
  PADDOCK_MACVM_FAKE_FREE=2G start nospace >/dev/null
  wait_new_event nospace hello "$hellos" 90 || { fail nospace "no hello on the second start"; return; }
  paddock app stop nospace >/dev/null 2>&1
  grep -q 'not enough space for the saved state' "$p/logs/shim.log" || { fail nospace "no skip line"; return; }
  if tail -3 "$p/logs/events.jsonl" | grep -q 'stopped without save'; then
    fail nospace "the guest did not shut down: $(grep '"kind":"error"' "$p/logs/events.jsonl" | tail -1)"; return
  fi
  grep -q '"clean": true' "$p/stamp.json" || { fail nospace "the stamp is not clean: $(grep '"kind":"error"' "$p/logs/events.jsonl" | tail -1)"; return; }
  [ -f "$p/state.vzs" ] && { fail nospace "a state.vzs exists"; return; }
  pass nospace
}

check_console() {
  local p="$profiles/console/default"
  start console >/dev/null
  wait_event console hello 90 || { fail console "no hello"; return; }
  paddock app stop console >/dev/null 2>&1
  [ -s "$p/logs/console.log" ] || { fail console "console.log is empty ($(grep -m1 'console' "$p/logs/agent.log" 2>/dev/null))"; return; }
  grep -q 'paddock-guest .* starts' "$p/logs/console.log" || { fail console "no start line of the agent: $(grep -m1 'console' "$p/logs/agent.log" 2>/dev/null)"; return; }
  grep -q 'ProductVersion' "$p/logs/console.log" || { fail console "no sw_vers line"; return; }
  echo "INFO console: $(grep -m1 'ProductVersion' "$p/logs/console.log" | tr -s '\t ' ' ')"
  pass console
}

check_format() {
  local f="$tmp/format.img" out disk
  mkfile -n 1g "$f" || { fail format "mkfile"; return; }
  out=$(hdiutil attach -imagekey diskimage-class=CRawDiskImage -nomount "$f" 2>&1) || { fail format "attach: $out"; return; }
  disk=$(echo "$out" | awk '/^\/dev\/disk[0-9]+[[:space:]]*$/ {print $1; exit} /^\/dev\/disk[0-9]+ / {print $1; exit}')
  [ -n "$disk" ] || { fail format "no disk in: $out"; return; }
  if ! diskutil partitionDisk "$disk" 1 GPT APFS Data 100% >"$tmp/format.log" 2>&1; then
    hdiutil detach "$disk" -force >/dev/null 2>&1
    fail format "partitionDisk as $(id -un): $(tail -2 "$tmp/format.log" | tr '\n' ' ')"; return
  fi
  hdiutil detach "$disk" >/dev/null 2>&1 || hdiutil detach "$disk" -force >/dev/null 2>&1 || { fail format "detach"; return; }
  [ "$(dd if="$f" bs=1 skip=512 count=8 2>/dev/null)" = "EFI PART" ] || { fail format "no GPT in the file"; return; }
  pass format
}

check_datadisk() {
  local p="$profiles/datadisk/default"
  start datadisk >/dev/null
  wait_event datadisk hello 90 || { fail datadisk "no hello"; return; }
  [ "$(dd if="$p/data.img" bs=1 skip=512 count=8 2>/dev/null)" = "EFI PART" ] || { fail datadisk "data.img has no GPT"; return; }
  local i=0
  while [ "$i" -lt 120 ] && ! grep -q 'the data disk is prepared' "$p/logs/events.jsonl" 2>/dev/null; do sleep 1; i=$((i + 1)); done
  if ! grep -q 'the data disk is prepared' "$p/logs/events.jsonl"; then
    copy_info datadisk
    fail datadisk "not prepared in 120 s: $(grep -m2 '"error"' "$p/logs/events.jsonl")"; return
  fi
  grep -q '"kind":"dialog"' "$p/logs/events.jsonl" && { fail datadisk "a dialog came up: $(grep -m1 '"kind":"dialog"' "$p/logs/events.jsonl")"; return; }
  grep -q 'not formatted on the host' "$p/logs/shim.log" && { fail datadisk "the host format failed: $(grep -m1 'not formatted on the host' "$p/logs/shim.log")"; return; }
  grep -q '"data_prepared": true' "$p/stamp.json" || { fail datadisk "the stamp does not say data_prepared"; return; }
  paddock app status datadisk 2>/dev/null | grep -q '^data disk *prepared' || { fail datadisk "status: $(paddock app status datadisk 2>&1 | grep '^data disk')"; return; }
  grep -q '"text":"copying the home: ' "$p/logs/events.jsonl" || { fail datadisk "no copy progress in events.jsonl"; return; }
  paddock app stop datadisk >/dev/null 2>&1
  copy_info datadisk
  # A later start verifies nothing new on the host: no second prepare.
  local hellos; hellos=$(count_event datadisk hello)
  start datadisk >/dev/null
  wait_new_event datadisk hello "$hellos" 90 || { fail datadisk "no hello on the second start"; return; }
  sleep 5
  [ "$(grep -c 'preparing the data disk: the home' "$p/logs/events.jsonl")" = 1 ] || { fail datadisk "the second start sent data.prepare again"; return; }
  paddock app stop datadisk >/dev/null 2>&1
  local out disk
  out=$(hdiutil attach -imagekey diskimage-class=CRawDiskImage -nomount "$p/data.img" 2>&1)
  disk=$(echo "$out" | awk '/^\/dev\/disk[0-9]+[[:space:]]/ {print $1; exit}')
  echo "INFO datadisk: $(diskutil apfs list 2>/dev/null | grep -A30 "Physical Store ${disk#/dev/}" | grep -E 'FileVault|Encrypted|Name:' | tr -s ' ' | tr '\n' ';')"
  [ -n "$disk" ] && hdiutil detach "$disk" -force >/dev/null 2>&1
  pass datadisk
}

check_dialog() {
  local p="$profiles/dialog/default"
  # An old profile: the folder and a blank, all-zero data.img (no GPT) of
  # the size of a new one, made before the first start. The host formats
  # only a data.img that does not exist, so this one stays blank.
  mkdir -p "$p" && chmod 700 "$profiles/dialog" "$p"
  rm -f "$p/data.img"
  mkfile -n 20g "$p/data.img" && chmod 600 "$p/data.img" || { fail dialog "mkfile"; return; }
  [ "$(dd if="$p/data.img" bs=512 count=34 2>/dev/null | tr -d '\000' | wc -c | tr -d ' ')" = 0 ] || { fail dialog "data.img is not blank"; return; }
  start dialog >/dev/null
  wait_event dialog hello 120 || { fail dialog "no hello"; return; }
  local i=0
  while [ "$i" -lt 120 ] && ! grep -qE 'the data disk is prepared|data disk was not prepared' "$p/logs/events.jsonl"; do sleep 1; i=$((i + 1)); done
  grep -q 'the data disk is prepared' "$p/logs/events.jsonl" || { fail dialog "the blank disk was not prepared: $(grep -m1 'data disk was not prepared' "$p/logs/events.jsonl")"; return; }
  # A dialog of the boot is pressed before the prepare, so its line is in.
  sleep 5
  local seen=0
  grep -q '"kind":"dialog"' "$p/logs/events.jsonl" && seen=1
  paddock app stop dialog >/dev/null 2>&1
  [ "$(dd if="$p/data.img" bs=1 skip=512 count=8 2>/dev/null)" = "EFI PART" ] || { fail dialog "data.img has no GPT after the prepare"; return; }
  if [ "$seen" = 0 ]; then
    if grep -q 'a disk dialog of' "$p/logs/agent.log" 2>/dev/null; then
      fail dialog "the agent saw a dialog but sent no line: $(grep -m1 'a disk dialog of' "$p/logs/agent.log")"; return
    fi
    local guest
    guest=$(sed -n 's/.*ProductVersion:[[:space:]]*//p' "$p/logs/console.log" 2>/dev/null | head -1)
    skip dialog "macOS ${guest:-?} in the guest showed no disk dialog for the blank data disk (no dialog line in events.jsonl or agent.log); the agent partitioned the disk"
    return
  fi
  grep '"kind":"dialog"' "$p/logs/events.jsonl" | grep -q 'dismissed' || { fail dialog "not dismissed: $(grep -m1 '"kind":"dialog"' "$p/logs/events.jsonl")"; return; }
  pass dialog
}

check_locksettings() {
  local p="$profiles/locksettings/default"
  start locksettings >/dev/null
  wait_event locksettings hello 90 || { fail locksettings "no hello"; return; }
  wait_prepared locksettings 120 || { fail locksettings "the data disk was not prepared in 120 s"; return; }
  # The agent log reaches the host with the quit.
  paddock app stop locksettings >/dev/null 2>&1
  local missing=""
  for s in no-sleep no-screen-saver no-screen-lock no-screen-saver-any-host no-saver-password no-saver-password-delay no-login-window-saver no-auto-logout; do
    grep -q "machine step $s: proven true" "$p/logs/agent.log" 2>/dev/null || missing="$missing $s"
  done
  [ -z "$missing" ] || { fail locksettings "not proven:$missing ($(grep -m3 'machine step' "$p/logs/agent.log" 2>/dev/null | tr '\n' ' '))"; return; }
  pass locksettings
}

check_noapp() {
  local p="$profiles/noapp/default"
  start noapp >/dev/null
  wait_event noapp hello 90 || { fail noapp "no hello"; return; }
  wait_prepared noapp 120 || { fail noapp "the data disk was not prepared in 120 s"; return; }
  sleep 5
  local t0 t1
  t0=$(date +%s)
  paddock app stop noapp >"$tmp/noapp.stop" 2>&1
  t1=$(date +%s)
  [ $((t1 - t0)) -le 40 ] || { fail noapp "the stop took $((t1 - t0)) s"; return; }
  grep -q 'does not run in the guest' "$p/logs/events.jsonl" || echo "INFO noapp: the app ran, so the answer was not tested: $(grep -m1 '"kind":"quit"' "$p/logs/events.jsonl")"
  grep -q '"text":"quit with save"' "$p/logs/events.jsonl" || { fail noapp "no save: $(grep '"kind":"stop"' "$p/logs/events.jsonl" | tail -1)"; return; }
  [ -f "$p/state.vzs" ] || { fail noapp "no state.vzs"; return; }
  pass noapp
}

check_input() {
  local p="$profiles/input/default"
  start input >/dev/null
  wait_event input hello 90 || { fail input "no hello"; return; }
  sleep 10
  paddock app stop input >/dev/null 2>&1
  [ -f "$p/logs/agent.log" ] || { fail input "no agent.log: $(grep '"kind":"files"' "$p/logs/events.jsonl" | tail -1)"; return; }
  grep -q 'cannot select the input source' "$p/logs/agent.log" && { fail input "$(grep -m1 'cannot select the input source' "$p/logs/agent.log")"; return; }
  local most
  most=$(grep -o 'the input source [^ ]* is not available' "$p/logs/agent.log" | sort | uniq -c | sort -rn | awk 'NR==1 {print $1}')
  [ "${most:-0}" -le 1 ] || { fail input "the fallback line came $most times"; return; }
  grep -m1 'is not available in the guest' "$p/logs/agent.log" | sed 's/^/INFO input: /'
  pass input
}

# control PROFILE LINE: one line to the control socket of the session,
# the answer on stdout.
control() {
  local sock
  sock=$(sed -n 's/.*"control": *"\([^"]*\)".*/\1/p' "$root/sessions/$1--default.json" 2>/dev/null | head -1)
  [ -n "$sock" ] || { echo "error no control socket for $1"; return 1; }
  printf '%s\n' "$2" | nc -U "$sock"
}

# health_locked PROFILE: `true` or `false` from the health of the record.
health_locked() {
  grep -o '"locked": *[a-z]*' "$root/sessions/$1--default.json" 2>/dev/null | head -1 | sed 's/.*: *//'
}

# screen_events PROFILE: the texts of the `screen` events, on one line.
screen_events() {
  grep '"kind":"screen"' "$profiles/$1/default/logs/events.jsonl" 2>/dev/null | sed 's/.*"text":"\([^"]*\)".*/\1/' | tr '\n' ';'
}

# bench_start PROFILE: a `--bench` session whose data disk is prepared.
bench_start() {
  PADDOCK_MACVM_BENCH=1 start "$1" --bench >/dev/null
  wait_event "$1" hello 90 || { echo "no hello"; return 1; }
  wait_prepared "$1" 120 || { echo "the data disk was not prepared in 120 s"; return 1; }
}

check_lock() {
  local p="$profiles/lock/default" ev="$profiles/lock/default/logs/events.jsonl" why i=0
  why=$(bench_start lock) || { fail lock "$why"; return; }
  sleep 5
  control lock "bench lock" | grep -q '^ok' || { fail lock "the bench lock line was refused: $(control lock 'bench lock' 2>&1)"; return; }
  while [ "$i" -lt 10 ] && ! grep -q '"text":"the guest screen locked at' "$ev"; do sleep 1; i=$((i + 1)); done
  if ! grep -q '"text":"the guest screen locked at' "$ev"; then
    fail lock "no lock within 10 s of bench lock: $(grep -m1 'bench.lock' "$ev")"; return
  fi
  i=0
  while [ "$i" -lt 15 ]; do
    grep -q '"text":"the guest screen is unlocked' "$ev" && [ "$(health_locked lock)" = false ] && break
    sleep 1; i=$((i + 1))
  done
  echo "INFO lock: $(screen_events lock)"
  grep -q '"text":"the guest screen is unlocked' "$ev" || { fail lock "not unlocked within 15 s: $(screen_events lock)"; return; }
  [ "$(health_locked lock)" = false ] || { fail lock "the health of the record says locked: $(health_locked lock)"; return; }
  local pw
  pw=$(cat "$XDG_STATE_HOME/paddock/keyring/macvm/guest-password" 2>/dev/null)
  if [ -n "$pw" ] && grep -qF -- "$pw" "$ev" "$p/logs/shim.log" 2>/dev/null; then
    fail lock "the guest password is in events.jsonl or shim.log"; return
  fi
  control lock "bench launch /System/Applications/TextEdit.app" | grep -q '^ok' || { fail lock "bench launch refused"; return; }
  i=0
  while [ "$i" -lt 20 ] && ! control lock "window list" | grep -q '"id"'; do sleep 1; i=$((i + 1)); done
  control lock "window list" | grep -q '"id"' || { fail lock "no TextEdit window listed within 20 s after the unlock"; return; }
  pass lock
}

# keychain_ok NAME: the `bench keychain` event says the login keychain of
# the moved home is unlocked, is the default and is in the search list.
keychain_ok() {
  local ev="$profiles/keychain/default/logs/events.jsonl" before i=0 line
  # The login keychain in the home on the data disk.
  local want=/Volumes/paddock/home
  want="$want/Library/Keychains/login.keychain-db"
  before=$(grep -c '"text":"bench: status' "$ev")
  control keychain "bench keychain" | grep -q '^ok' || { echo "the bench keychain line was refused"; return 1; }
  while [ "$i" -lt 15 ] && [ "$(grep -c '"text":"bench: status' "$ev")" -le "$before" ]; do sleep 1; i=$((i + 1)); done
  line=$(grep '"text":"bench: status' "$ev" | tail -1)
  [ -n "$line" ] || { echo "no bench keychain event within 15 s: $(grep -m1 'bench.keychain' "$ev")"; return 1; }
  echo "INFO keychain $1: $(printf '%s' "$line" | sed 's/.*"text":"\([^"]*\)".*/\1/')"
  printf '%s' "$line" | grep -q "bench: status unlocked $want" || { echo "the login keychain is not unlocked"; return 1; }
  printf '%s' "$line" | grep -q "default $want" || { echo "the default keychain is not the one of the moved home"; return 1; }
  printf '%s' "$line" | grep -q "search $want" || { echo "the search list does not start with the keychain of the moved home"; return 1; }
}

check_keychain() {
  local p="$profiles/keychain/default" ev="$profiles/keychain/default/logs/events.jsonl" why
  why=$(bench_start keychain) || { fail keychain "$why"; return; }
  sleep 5
  grep '"kind":"keychain"' "$ev" | grep -v 'bench:' | sed 's/^/INFO keychain unlock: /'
  grep -q '"text":"the login keychain .*unlocked' "$ev" || { fail keychain "no keychain event of the unlock before the app: $(grep -m1 '"kind":"keychain"' "$ev")"; return; }
  why=$(keychain_ok first) || { fail keychain "$why"; return; }
  paddock app stop keychain >/dev/null 2>&1
  grep -q '"text":"quit with save"' "$ev" || { fail keychain "the quit did not save"; return; }
  local hellos; hellos=$(count_event keychain hello)
  PADDOCK_MACVM_BENCH=1 start keychain --bench >/dev/null
  wait_new_event keychain hello "$hellos" 30 || { fail keychain "no hello after the warm start"; return; }
  sleep 5
  why=$(keychain_ok warm) || { fail keychain "after the warm start: $why"; return; }
  pass keychain
}

# js CODE ARGS: run JavaScript for Automation on the arguments (`argv`),
# the JSON parser of this script.
js() { local code=$1; shift; osascript -l JavaScript -e "function run(argv) { $code }" "$@"; }

# app_window PROFILE APP: a session of the profile with the guest app APP
# and its first window; prints the window id.
app_window() {
  # The bench door (`bench launch`) needs a session started with --bench.
  PADDOCK_MACVM_BENCH=1 start "$1" --bench >/dev/null
  wait_event "$1" hello 90 || { echo "no hello"; return 1; }
  # The guest refuses an app launch while it prepares the data disk.
  wait_prepared "$1" 120 || { echo "the data disk was not prepared in 120 s"; return 1; }
  local answer; answer=$(control "$1" "bench launch $2")
  case "$answer" in ok*) ;; *) echo "bench launch refused: $answer"; return 1 ;; esac
  local i=0 id=""
  while [ "$i" -lt 150 ]; do
    id=$(control "$1" "window list" | sed -n 's/^ok //p' | tr '{' '\n' | sed -n 's/.*"id":\([0-9]*\)[,}].*/\1/p' | head -1)
    [ -n "$id" ] && break
    sleep 0.1; i=$((i + 1))
  done
  [ -n "$id" ] || { echo "$2 showed no window in 15 s"; return 1; }
  echo "$id"
}

# frames PROFILE ID: the `window frames` JSON.
frames() { control "$1" "window frames $2" | sed -n 's/^ok //p'; }

# apart JSON: the largest difference of the host and guest frames, points.
apart() { js 'var o = JSON.parse(argv[0]); var d = 0; for (var i = 0; i < 4; i++) d = Math.max(d, Math.abs(o.host[i] - o.guest[i])); return d.toFixed(2);' "$1"; }

check_drag() {
  local id ev="$profiles/drag/default/logs/events.jsonl"
  id=$(app_window drag /System/Applications/TextEdit.app) || { fail drag "$id"; return; }
  local before i=0 worst=0
  before=$(grep -c 'the guest followed the host move' "$ev" 2>/dev/null)
  before=${before:-0}
  while [ "$i" -lt 20 ]; do
    local nx=$(( 100 + (i % 5) * 40 )) ny=$(( 100 + (i / 5) * 30 )) n=0 s=0
    control drag "window host-move $id $nx $ny" | grep -q '^ok' || { fail drag "the host-move line was refused"; return; }
    while [ "$(grep -c 'the guest followed the host move' "$ev")" -le $(( before + i )) ]; do
      sleep 0.02; n=$((n + 1))
      [ "$n" -lt 100 ] || { fail drag "move $i: the guest did not follow within 2 s ($(grep '"kind":"window"' "$ev" | tail -1))"; return; }
    done
    while [ "$s" -lt 5 ]; do
      local d; d=$(apart "$(frames drag "$id")")
      if [ "$(js 'return Number(argv[0]) > Number(argv[1]) ? "1" : "0";' "$d" "$worst")" = 1 ]; then worst=$d; fi
      sleep 0.04; s=$((s + 1))
    done
    i=$((i + 1))
  done
  local sorted p50 p95
  sorted=$(grep 'the guest followed the host move' "$ev" | tail -20 | sed -n 's/.*"ms":\([0-9.]*\).*/\1/p' | sort -n)
  p50=$(printf '%s\n' "$sorted" | sed -n '10p')
  p95=$(printf '%s\n' "$sorted" | sed -n '19p')
  echo "INFO drag: p50 $p50 ms, p95 $p95 ms, host and guest at most $worst points apart after the moves"
  [ "$(js 'return Number(argv[0]) < 50 ? "1" : "0";' "$p95")" = 1 ] || { fail drag "p95 $p95 ms, not below 50"; return; }
  [ "$(js 'return Number(argv[0]) <= 2 ? "1" : "0";' "$worst")" = 1 ] || { fail drag "host and guest $worst points apart after a move"; return; }
  pass drag
}

# radius_of PROFILE APP MINBAR: the clip radius of the first window of APP
# is within 1 point of the measured one; MINBAR is the least title bar.
radius_of() {
  local id i=0 j=""
  id=$(app_window "$1" "$2") || { echo "$id"; return 1; }
  while [ "$i" -lt 50 ]; do
    j=$(frames "$1" "$id")
    [ "$(js 'var o = JSON.parse(argv[0]); return o.measured == null ? "0" : "1";' "$j")" = 1 ] && break
    sleep 0.1; i=$((i + 1))
  done
  [ "$i" -lt 50 ] || { echo "no measured radius in 5 s (no Screen Recording row for the agent in the guest?): $j"; return 1; }
  echo "INFO radius: $2: $j" >&2
  js 'var o = JSON.parse(argv[0]); if (o.title_bar < Number(argv[1])) return "the title bar is " + o.title_bar + ", not a toolbar window"; return Math.abs(o.radius - o.measured) <= 1 ? "" : "the clip radius " + o.radius + " is not within 1 point of " + o.measured;' "$j" "$3"
}

check_radius() {
  local why
  why=$(radius_of radius /System/Applications/TextEdit.app 0) || { fail radius "TextEdit: $why"; return; }
  [ -z "$why" ] || { fail radius "TextEdit: $why"; return; }
  paddock app stop radius >/dev/null 2>&1
  why=$(radius_of radius2 "/System/Applications/Utilities/Activity Monitor.app" 40) || { fail radius "Activity Monitor: $why"; return; }
  [ -z "$why" ] || { fail radius "Activity Monitor: $why"; return; }
  pass radius
}

# menu_path PROFILE TITLE: the path of the menu item with that title.
menu_path() {
  local i=0 p=""
  while [ "$i" -lt 100 ]; do
    p=$(js 'var o = JSON.parse(argv[0]); var hit = ""; function walk(l) { l.forEach(function (m) { if (!hit && m.title == argv[1]) hit = m.path.join("."); walk(m.children || []); }); } walk(o.tree || []); return hit;' "$(control "$1" "window menu" | sed -n 's/^ok //p')" "$2")
    [ -n "$p" ] && { echo "$p"; return 0; }
    sleep 0.1; i=$((i + 1))
  done
  return 1
}

# full_state PROFILE ID WANT: wait up to 5 s for both full screen states
# to be WANT (true or false); prints the last `window frames`.
full_state() {
  local i=0 j=""
  while [ "$i" -lt 50 ]; do
    j=$(frames "$1" "$2")
    [ "$(js 'var o = JSON.parse(argv[0]); return String(o.full_screen && o.guest_full_screen) == argv[1] && String(o.full_screen || o.guest_full_screen) == argv[1] ? "1" : "0";' "$j" "$3")" = 1 ] && { echo "$j"; return 0; }
    sleep 0.1; i=$((i + 1))
  done
  echo "$j"; return 1
}

check_fullscreen() {
  local id path j
  id=$(app_window fullscreen /System/Applications/TextEdit.app) || { fail fullscreen "$id"; return; }
  path=$(menu_path fullscreen "Enter Full Screen") || { fail fullscreen "no Enter Full Screen item in the menu bar of TextEdit"; return; }
  control fullscreen "window menu-press $path" | grep -q '^ok' || { fail fullscreen "the press was refused"; return; }
  j=$(full_state fullscreen "$id" true) || { fail fullscreen "not in full screen on both sides in 5 s: $j"; return; }
  local why
  why=$(js 'var o = JSON.parse(argv[0]); if (o.radius != 0) return "the clip radius is " + o.radius; if (o.host[0] != 0 || o.host[1] != 0) return "the host window is at " + o.host[0] + "," + o.host[1] + ", not at origin zero"; for (var i = 2; i < 4; i++) if (Math.abs(o.host[i] - o.guest[i]) > 2) return "the host window is " + o.host[2] + "x" + o.host[3] + ", the guest window " + o.guest[2] + "x" + o.guest[3]; return "";' "$j")
  [ -z "$why" ] || { fail fullscreen "$why"; return; }
  path=$(menu_path fullscreen "Exit Full Screen") || { fail fullscreen "no Exit Full Screen item in the menu bar of TextEdit"; return; }
  control fullscreen "window menu-press $path" | grep -q '^ok' || { fail fullscreen "the exit press was refused"; return; }
  j=$(full_state fullscreen "$id" false) || { fail fullscreen "still in full screen after 5 s: $j"; return; }
  sleep 1
  j=$(frames fullscreen "$id")
  [ "$(js 'return Number(argv[0]) <= 2 ? "1" : "0";' "$(apart "$j")")" = 1 ] || { fail fullscreen "host and guest apart after the exit: $j"; return; }
  pass fullscreen
}

# json_text TEXT: TEXT as the inside of a JSON string.
json_text() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# guest_child PROFILE ARG...: start a child of the agent in the guest
# (`bench.spawn_child`, a --bench session only).
guest_child() {
  local name=$1 argv="" a; shift
  for a in "$@"; do argv="$argv${argv:+,}\"$(json_text "$a")\""; done
  control "$name" "bench op {\"op\":\"bench.spawn_child\",\"argv\":[$argv]}"
}

# guest_echo_server PORT: a one-shot HTTP server on the guest loopback
# that answers with the request line it got.
guest_echo_server() {
  local f="/tmp/paddock-login-$1"
  printf '%s' "rm -f $f; mkfifo $f; /usr/bin/nc -l 127.0.0.1 $1 < $f | (IFS= read -r l; l=\$(printf %s \"\$l\" | tr -d '\\r'); b=\"got \$l\"; printf 'HTTP/1.0 200 OK\\r\\nContent-Length: %d\\r\\nConnection: close\\r\\n\\r\\n%s' \${#b} \"\$b\") > $f; rm -f $f"
}

check_login() {
  local ev="$profiles/login/default/logs/events.jsonl" why port other i out lsofs
  port=$((47000 + RANDOM % 900)); other=$((port + 1))
  mkdir -p "$XDG_CONFIG_HOME/paddock"
  printf '[host]\nopen-url = "allow"\n' >"$XDG_CONFIG_HOME/paddock/config.toml"
  why=$(PADDOCK_MACVM_LINK_DRY=1 bench_start login) || { fail login "$why"; return; }
  sleep 5
  grep -q 'the link helper is registered' "$profiles/login/default/logs/agent.log" 2>/dev/null ||
    echo "INFO login: agent.log has no line that the link helper is registered (the log comes with the quit)"
  guest_child login /bin/sh -c "$(guest_echo_server "$port")" | grep -q '^ok' || { fail login "the guest server did not start: $(guest_child login /usr/bin/true 2>&1)"; return; }
  guest_child login /bin/sh -c "$(guest_echo_server "$other")" >/dev/null
  sleep 2
  local url="http://127.0.0.1:$port/start?redirect_uri=http%3A%2F%2F127.0.0.1%3A$port%2Fcallback&state=check"
  guest_child login /usr/bin/open "$url" | grep -q '^ok' || { fail login "open of the login link in the guest was refused"; return; }
  i=0
  while [ "$i" -lt 30 ] && ! grep -q "\"text\":\"the login callback port $port listens on" "$ev"; do sleep 1; i=$((i + 1)); done
  grep '"kind":"link"\|"kind":"login"' "$ev" | sed 's/^/INFO login: /'
  grep -q '"text":"took the link http://127.0.0.1' "$ev" || { fail login "no link event: the guest link did not reach the host through the link helper"; return; }
  grep -q "\"text\":\"the login callback port $port listens on" "$ev" || { fail login "the callback port $port did not listen within 30 s"; return; }
  lsofs=$(lsof -nP -iTCP:"$port" -sTCP:LISTEN 2>/dev/null | awk 'NR > 1 {print $9}' | sort -u | tr '\n' ' ')
  echo "INFO login: listening on $lsofs"
  for a in $lsofs; do
    case "$a" in
      127.0.0.1:"$port" | "[::1]:$port") ;;
      *) fail login "the callback port listens on $a, not only on the loopback"; return ;;
    esac
  done
  out=$(curl -s --max-time 10 "http://127.0.0.1:$port/callback?code=x")
  echo "INFO login: curl got: $out"
  [ "$out" = "got GET /callback?code=x HTTP/1.1" ] || { fail login "the guest server did not get the callback: $out"; return; }
  if curl -s --max-time 3 -o /dev/null "http://127.0.0.1:$other/"; then
    fail login "the guest port $other, which no link named, answers on the host"; return
  fi
  sleep 15
  if curl -s --max-time 3 -o /dev/null "http://127.0.0.1:$port/"; then
    fail login "the callback port $port still answers 15 s after the callback"; return
  fi
  grep -q "\"text\":\"the login callback port $port closed after its callback" "$ev" || { fail login "no line that the callback port closed after its callback"; return; }
  pass login
}

run_check() {
  if ! golden_ready; then
    skip "$1" "${not_ready:-the golden clone failed}"
    return
  fi
  "check_$1"
  stop_sessions
}

case "$what" in
  build) build ;;
  all)
    build
    zombies_before=$(zombies | grep -c .)
    # PADDOCK_CHECKS: a subset, in this order.
    for c in ${PADDOCK_CHECKS:-fresh warm kill9 slots suspend instances gcdata hup twice nospace console datadisk dialog noapp input locksettings lock keychain drag radius fullscreen login}; do run_check "$c"; done
    check_format
    check_zombies
    ;;
  format)
    check_format
    ;;
  fresh | warm | kill9 | slots | suspend | instances | gcdata | hup | twice | nospace | console | datadisk | dialog | noapp | input | locksettings | lock | keychain | drag | radius | fullscreen | login)
    build
    run_check "$what"
    ;;
  *) echo "usage: $0 build|all|format|fresh|warm|kill9|slots|suspend|instances|gcdata|hup|twice|nospace|console|datadisk|dialog|noapp|input|locksettings|lock|keychain|drag|radius|fullscreen" >&2; exit 2 ;;
esac
echo "macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion)), $(sysctl -n machdep.cpu.brand_string 2>/dev/null)"
[ "$fails" = 0 ]
