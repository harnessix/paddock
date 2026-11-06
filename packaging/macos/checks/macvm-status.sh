#!/usr/bin/env bash
# Observability of the macOS guest, checked on a Mac. `paddock app
# status`, the four logs of a profile and their secrets, `paddock app log
# --follow`, `paddock doctor --report` and the `macvm` block of `paddock
# doctor`.
#
# Run this script on an Apple silicon Mac with a sealed
# golden (`paddock macvm setup`). Every check runs in a temporary
# data folder with a clone of that golden (APFS, no space) and a scratch
# profile that runs TextEdit, so the real profiles are never touched.
# Each check prints one line, `PASS name`, `FAIL name: reason` or `SKIP
# name: reason`, and the script exits 0 when every check that ran passed.
#
# Usage:
#   PADDOCK_MACVM_GOLDEN=~/.local/share/paddock/macvm/golden.noindex \
#     packaging/macos/checks/macvm-status.sh all
#   packaging/macos/checks/macvm-status.sh status-running   # one check
#
# What proves each check (the lines of this script and the files it reads):
#   build            cargo build of paddock and paddock-vm, the entitlement
#                    on paddock-vm, clippy, and the Linux tests of this
#                    part (macvm::status, macvm::events, macvm::logs,
#                    macvm::report, doctor::macvm).
#   status-running   with the session up: `paddock app status NAME --json`
#                    has state = running, health_age_s under 3, windows =
#                    1, data_free_gib over 0, and every path under `logs`
#                    exists (agent.log after the first quit only, so it
#                    is checked in `logs`). The text form has its eight
#                    rows.
#   status-stopped   after `paddock app stop`: state = stopped, last_end =
#                    quit with save, next_start = warm.
#   logs             logs/shim.log, logs/events.jsonl, logs/console.log
#                    and logs/agent.log exist after the quit, and none of
#                    them holds the session token. The token comes from
#                    PADDOCK_MACVM_PRINT_SECRETS=1, which makes `paddock
#                    app run` print it to its own stderr, nowhere else.
#   follow           `paddock app log NAME --events --follow` prints the
#                    `quit` line within 1 s of `paddock app stop`.
#   report           `paddock doctor --report --output FILE`: the file has
#                    the MACVM section with the profile, and grep for the
#                    token, the guest password (from the same test flag)
#                    and a clipboard marker copied during the session
#                    finds nothing. Also with --no-hosts.
#   doctor           `paddock doctor` has the `macvm` block with `ok` on
#                    the golden line and the `no host permission needed`
#                    line, and the profile with its last end.
#
# The script prints `paddock app status NAME` and `paddock doctor` at the end.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-status.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
export PATH="$here/target/debug:$PATH"
root="$XDG_DATA_HOME/paddock/macvm"
profiles="$root/profiles.noindex"
app="status-textedit"
profile="$profiles/$app/default"
events="$profile/logs/events.jsonl"
clip_marker="CLIPBOARD-MARKER-status-$(date +%s)"
token=""
password=""
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }

check_build() {
  if ! cargo build --bin paddock --bin paddock-vm >"$tmp/build.log" 2>&1; then
    fail build "cargo build: $(tail -3 "$tmp/build.log" | tr '\n' ' ')"; return
  fi
  if ! codesign --force --sign - --entitlements packaging/macos/paddock-vm.entitlements target/debug/paddock-vm >"$tmp/sign.log" 2>&1; then
    fail build "codesign of paddock-vm failed"; return
  fi
  if ! cargo clippy --all-targets -- -D warnings >"$tmp/clippy.log" 2>&1; then
    fail build "clippy: $(grep -m1 -A3 '^error' "$tmp/clippy.log" | tr '\n' ' ')"; return
  fi
  for t in macvm::status macvm::events macvm::logs macvm::report doctor::macvm; do
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

# wait_line FILE TEXT SECONDS: true when the file gets the text in time.
wait_line() {
  local file=$1 text=$2 secs=$3 i=0
  while [ "$i" -lt "$((secs * 10))" ]; do
    grep -qF "$text" "$file" 2>/dev/null && return 0
    sleep 0.1; i=$((i + 1))
  done
  return 1
}

# json FILE KEY...: one value of a JSON file, through python.
json() {
  local file=$1; shift
  python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))
for k in sys.argv[2:]:
    v = v[k] if isinstance(v, dict) else v[int(k)]
print(v if not isinstance(v, bool) else str(v).lower())
' "$file" "$@" 2>/dev/null
}

start_session() {
  PADDOCK_MACVM_PRINT_SECRETS=1 paddock app run "$app" --backend macvm --network none "$tmp/work" >"$tmp/run.out" 2>&1 &
  wait_line "$events" '"kind":"hello"' 90 || return 1
  # The app and its first window: TextEdit opens one untitled document.
  wait_line "$events" '"kind":"launch"' 60
  sleep 5
  token=$(sed -n 's/.*token=\([0-9a-f]*\).*/\1/p' "$tmp/run.out" | head -1)
  password=$(sed -n 's/.*password=\([^ ]*\).*/\1/p' "$tmp/run.out" | head -1)
  [ -n "$token" ]
}

check_status_running() {
  mkdir -p "$tmp/work"
  if ! start_session; then
    fail status-running "no session (see $profile/logs/ and $tmp/run.out)"; return
  fi
  # A clipboard marker through the session, for the report check: the
  # text goes to the guest pasteboard while a session window is key.
  printf '%s' "$clip_marker" | pbcopy
  osascript -e 'tell application "TextEdit (paddock)" to activate' >/dev/null 2>&1 || true
  sleep 2
  paddock app status "$app" --json >"$tmp/status.json" 2>"$tmp/status.err" || { fail status-running "status --json exited $?: $(cat "$tmp/status.err")"; return; }
  local state age windows free
  state=$(json "$tmp/status.json" state)
  age=$(json "$tmp/status.json" health_age_s)
  windows=$(json "$tmp/status.json" windows)
  free=$(json "$tmp/status.json" data_free_gib)
  [ "$state" = running ] || { fail status-running "state is $state"; return; }
  [ -n "$age" ] && [ "$age" -lt 3 ] || { fail status-running "health_age_s is $age"; return; }
  [ "$windows" = 1 ] || { fail status-running "windows is $windows (TextEdit has one document window)"; return; }
  python3 -c "import sys; sys.exit(0 if float('$free') > 0 else 1)" || { fail status-running "data_free_gib is $free"; return; }
  for key in shim events console boundary; do
    local p; p=$(json "$tmp/status.json" logs "$key")
    [ -f "$p" ] || { fail status-running "logs.$key $p does not exist"; return; }
  done
  paddock app status "$app" >"$tmp/status.txt" 2>&1
  for row in golden app guest host network shares logs; do
    grep -q "^$row " "$tmp/status.txt" || { fail status-running "the text form lacks the $row row"; return; }
  done
  grep -q "^$app .*running (pid" "$tmp/status.txt" || { fail status-running "the head line: $(head -1 "$tmp/status.txt")"; return; }
  pass status-running
}

check_follow() {
  [ -f "$events" ] || { skip follow "no session"; return; }
  paddock app log "$app" --events --follow >"$tmp/follow.out" 2>&1 &
  local fpid=$!
  sleep 1
  local t0 t1
  t0=$(python3 -c 'import time; print(time.time())')
  paddock app stop "$app" >"$tmp/stop.out" 2>&1 &
  while ! grep -q '  quit     ' "$tmp/follow.out" 2>/dev/null; do
    t1=$(python3 -c 'import time; print(time.time())')
    if python3 -c "import sys; sys.exit(0 if $t1 - $t0 > 5 else 1)"; then break; fi
    sleep 0.05
  done
  t1=$(python3 -c 'import time; print(time.time())')
  wait %2 2>/dev/null
  if ! grep -q '  quit     ' "$tmp/follow.out"; then
    kill "$fpid" 2>/dev/null; fail follow "no quit line in $tmp/follow.out"; return
  fi
  python3 -c "import sys; sys.exit(0 if $t1 - $t0 <= 1.0 else 1)" || { kill "$fpid" 2>/dev/null; fail follow "the quit line took $(python3 -c "print(round($t1 - $t0, 2))") s"; return; }
  # The follow ends by itself when the session is gone.
  local i=0
  while kill -0 "$fpid" 2>/dev/null && [ "$i" -lt 150 ]; do sleep 0.1; i=$((i + 1)); done
  if kill -0 "$fpid" 2>/dev/null; then kill "$fpid"; fail follow "--follow did not end with the session"; return; fi
  pass follow
}

check_status_stopped() {
  [ -f "$profile/stamp.json" ] || { skip status-stopped "no profile"; return; }
  paddock app status "$app" --json >"$tmp/status2.json" 2>&1 || { fail status-stopped "status exited $?"; return; }
  local state end next
  state=$(json "$tmp/status2.json" state)
  end=$(json "$tmp/status2.json" last_end)
  next=$(json "$tmp/status2.json" next_start)
  [ "$state" = stopped ] || { fail status-stopped "state is $state"; return; }
  [ "$end" = "quit with save" ] || { fail status-stopped "last_end is $end"; return; }
  [ "$next" = warm ] || { fail status-stopped "next_start is $next"; return; }
  pass status-stopped
}

check_logs() {
  [ -n "$token" ] || { skip logs "no token (the session did not start)"; return; }
  for f in shim.log events.jsonl console.log agent.log; do
    [ -f "$profile/logs/$f" ] || { fail logs "$profile/logs/$f is missing after the quit"; return; }
    if grep -qF "$token" "$profile/logs/$f"; then fail logs "$f holds the session token"; return; fi
    if [ -n "$password" ] && grep -qF "$password" "$profile/logs/$f"; then fail logs "$f holds the guest password"; return; fi
    if grep -qF "$clip_marker" "$profile/logs/$f"; then fail logs "$f holds the clipboard text"; return; fi
  done
  pass logs
}

check_report() {
  [ -f "$profile/stamp.json" ] || { skip report "no profile"; return; }
  paddock doctor --report --output "$tmp/report.txt" >"$tmp/report.out" 2>&1 || { fail report "doctor --report exited $?: $(cat "$tmp/report.out")"; return; }
  grep -q '^MACVM' "$tmp/report.txt" || { fail report "no MACVM section"; return; }
  grep -q "profile $app/default" "$tmp/report.txt" || { fail report "the profile is not in the section"; return; }
  for secret in "$token" "$password" "$clip_marker"; do
    [ -n "$secret" ] || continue
    if grep -qF "$secret" "$tmp/report.txt"; then fail report "the report holds a secret ($secret)"; return; fi
  done
  paddock doctor --report --no-hosts --output "$tmp/report2.txt" >/dev/null 2>&1 || { fail report "--no-hosts exited $?"; return; }
  grep -q 'host names replaced with host-N' "$tmp/report2.txt" || { fail report "no --no-hosts note"; return; }
  pass report
}

check_doctor() {
  paddock doctor >"$tmp/doctor.txt" 2>&1
  grep -q '^macvm:' "$tmp/doctor.txt" || { fail doctor "no macvm block"; return; }
  grep -q '^  ok       golden macOS' "$tmp/doctor.txt" || { fail doctor "the golden line is not ok: $(grep golden "$tmp/doctor.txt" | head -1)"; return; }
  grep -q 'no host permission needed' "$tmp/doctor.txt" || { fail doctor "no 'no host permission needed' line"; return; }
  grep -q "profiles: .*$app/default .* (clean)" "$tmp/doctor.txt" || { fail doctor "the profile with its end is missing: $(grep profiles "$tmp/doctor.txt")"; return; }
  grep -q 'slots: 0 of 2 in use' "$tmp/doctor.txt" || { fail doctor "the slots line: $(grep slots "$tmp/doctor.txt")"; return; }
  pass doctor
}

run() {
  case $1 in
    build) check_build ;;
    status-running) check_status_running ;;
    follow) check_follow ;;
    status-stopped) check_status_stopped ;;
    logs) check_logs ;;
    report) check_report ;;
    doctor) check_doctor ;;
    *) echo "unknown check $1"; exit 2 ;;
  esac
}

if [ "$what" = all ]; then
  check_build
  if golden_ready; then
    check_status_running
    check_follow
    check_status_stopped
    check_logs
    check_report
    check_doctor
  else
    for c in status-running follow status-stopped logs report doctor; do
      skip "$c" "set PADDOCK_MACVM_GOLDEN to a sealed golden folder"
    done
  fi
else
  if [ "$what" != build ]; then
    golden_ready || { echo "set PADDOCK_MACVM_GOLDEN to a sealed golden folder"; exit 2; }
    case $what in
      follow|status-stopped|logs|report) check_status_running ;;
    esac
  fi
  run "$what"
fi

echo
echo "---- paddock app status $app ----"
paddock app status "$app" 2>&1 || true
echo
echo "---- paddock doctor ----"
cat "$tmp/doctor.txt" 2>/dev/null || paddock doctor 2>&1 || true
echo
echo "macOS $(sw_vers -productVersion 2>/dev/null) on $(sysctl -n hw.model 2>/dev/null)"
echo "logs: $profile/logs/  run output: $tmp/run.out"
paddock app stop "$app" >/dev/null 2>&1 || true
[ "$fails" -eq 0 ]
