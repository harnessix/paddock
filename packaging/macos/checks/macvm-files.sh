#!/usr/bin/env bash
# Files and the clipboard of the macOS guest, checked on a Mac.
#
# Run this script on an Apple silicon Mac with a sealed
# golden image (`paddock macvm setup`). It builds the worktree, signs
# `paddock-vm` with its entitlement, and runs every check in a temporary
# data folder with a scratch profile that runs TextEdit, so it never
# touches the real paddock data or profiles. Each check prints one line:
# `PASS name` or `FAIL name: reason`, `SKIP` when it cannot run here. The
# script exits 0 when every check that ran passed.
#
# Usage:
#   packaging/macos/checks/macvm-files.sh build       # compile, clippy, the Linux-portable tests
#   packaging/macos/checks/macvm-files.sh all         # build plus every check
#   packaging/macos/checks/macvm-files.sh shares      # one check by name
#
# Name the golden with PADDOCK_MACVM_GOLDEN=/path/to/macvm/golden.noindex;
# the script clones it (APFS) into the temporary data folder.
#
# What proves each check (the lines of this script and the files it reads):
#   build          `cargo build`, `cargo clippy -D warnings`,
#                  `cargo test macvm::shares macvm::drop macvm::clipboard
#                  macvm::files_port guest::` exit 0 on macOS.
#   shares         a session with a read-write and a read-only root: the
#                  guest has three ws-/paddock shares under
#                  /Volumes/My Shared Files, `touch` on the read-only one
#                  fails in the guest, `touch` on the read-write one shows
#                  on the host within 1 s. (Needs the guest shell of the
#                  bench op `paddock macvm bench sh`; SKIP without it.)
#   env            a `.env` in a root: `paddock app run` exits 2 before any
#                  clone, and the output names the file and
#                  unprotected_workspaces.
#   clip-host-guest `pbcopy` on the host while TextEdit is key: the guest
#                  pasteboard has the text within 300 ms (events.jsonl gets
#                  "the host clipboard is in the guest").
#   clip-guest-host a guest copy (bench op) while key: `pbpaste` on the
#                  host has it within 300 ms.
#   clip-not-key   a host copy while the Finder is key: no clipboard event
#                  for 1 s; then focus TextEdit: the event within 300 ms.
#   concealed      a host item with org.nspasteboard.ConcealedType while
#                  key: no clipboard event for 1 s.
#   drop           a file dropped by a synthetic drag onto the host window:
#                  drop/<time>-1/ holds the copy and events.jsonl has
#                  "dropped as open" within 2 s. (The synthetic drag needs
#                  the host window; SKIP without `paddock macvm bench
#                  drag`.)
#   download       `curl -o ~/Downloads/x` in the guest through the proxy:
#                  the file is in profile/downloads/ within 1 s.
#   limits         65 transfers in a minute through the bench op: the 65th
#                  is refused and events.jsonl has "more than 64 transfers".
#   configure      the golden's configure list has `downloads-link` and
#                  `readlink ~/Downloads` in the guest prints the share
#                  (through `paddock macvm bench configure-verify`).
#
# The link itself is made in the stopped guest by the offline trust
# (src/macvm/offline_link.rs, `macvm-offline.sh downloads`). The
# `downloads-link` configure step only runs `test -L` and `readlink` on
# ~/Downloads, which macOS does not guard with a Files and Folders prompt.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-files.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profiles="$root/profiles.noindex"
# The scratch workspace folders live under the home: the temporary folder
# of the system is one the share builder refuses.
work="$HOME/.paddock-files-work-$$"
mkdir -p "$work/rw" "$work/ro"
app=textedit-files
events="$profiles/$app/default/logs/events.jsonl"

cleanup() {
  paddock app stop "$app" --wait 30 >/dev/null 2>&1
  for rec in "$root"/sessions/*.json; do
    [ -f "$rec" ] || continue
    pid=$(sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$rec" | head -1)
    [ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null
  done
  sleep 1
  pgrep -f "paddock-vm app --profile $profiles" | xargs -r kill -9 2>/dev/null
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" "$work" 2>/dev/null
  rm -rf "$tmp" "$work"
}
trap cleanup EXIT

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
  for t in macvm::shares macvm::drop macvm::clipboard macvm::files_port macvm::text guest::; do
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

bench() { paddock macvm bench --help >/dev/null 2>&1; }

# wait_line FILE TEXT SECONDS: true when the file gets the text in time.
wait_line() {
  local file=$1 text=$2 secs=$3 i=0
  while [ "$i" -lt "$((secs * 10))" ]; do
    grep -qF "$text" "$file" 2>/dev/null && return 0
    sleep 0.1; i=$((i + 1))
  done
  return 1
}

count_lines() { grep -cF "$2" "$1" 2>/dev/null || echo 0; }

# --network egress: the check needs the network but is not about the
# firewall, so no question may wait for a person (macvm-firewall.sh is the
# check of the panel).
start_session() {
  paddock app run "$app" --backend macvm --network egress "$work/rw" --ro "$work/ro" >"$tmp/run.out" 2>&1 &
  wait_line "$events" '"kind":"hello"' 90
}

focus() { osascript -e "tell application \"$1\" to activate" >/dev/null 2>&1; sleep 0.5; }

check_shares() {
  if ! bench; then skip shares "needs the guest shell bench op (paddock macvm bench sh)"; return; fi
  grep -q 'shares    2 folder' "$tmp/run.out" || { fail shares "the start lines do not list 2 folders"; return; }
  if ! paddock macvm bench sh "$app" -- touch '/Volumes/My Shared Files/ws-2/x' >/dev/null 2>&1; then
    :
  else
    fail shares "touch on the read-only share succeeded in the guest"; return
  fi
  paddock macvm bench sh "$app" -- touch '/Volumes/My Shared Files/ws-1/y' >/dev/null 2>&1
  sleep 1
  [ -f "$work/rw/y" ] || { fail shares "the touch on ws-1 did not show on the host"; return; }
  pass shares
}

check_env() {
  mkdir -p "$work/secret"
  echo "KEY=1" >"$work/secret/.env"
  paddock app run "${app}-env" --backend macvm --network none "$work/secret" >"$tmp/env.out" 2>&1
  code=$?
  if [ "$code" -ne 2 ] && [ "$code" -ne 1 ]; then fail env "exit $code, not a refusal"; return; fi
  grep -q "$work/secret/.env" "$tmp/env.out" || { fail env "the file is not named: $(head -3 "$tmp/env.out")"; return; }
  grep -q unprotected_workspaces "$tmp/env.out" || { fail env "no unprotected_workspaces hint"; return; }
  [ -d "$profiles/${app}-env/default" ] && { fail env "a profile folder was made before the refusal"; return; }
  pass env
}

check_clip_host_guest() {
  focus TextEdit
  before=$(count_lines "$events" "the host clipboard is in the guest")
  printf 'files host text %s' "$$" | pbcopy
  if wait_line_count "$events" "the host clipboard is in the guest" "$before" 0.3; then pass clip-host-guest; else fail clip-host-guest "no clipboard.set within 300 ms"; fi
}

# wait_line_count FILE TEXT BEFORE SECONDS: true when the count grows.
wait_line_count() {
  local file=$1 text=$2 before=$3 secs=$4 i=0 n
  n=$(python3 -c "print(int($secs*20))")
  while [ "$i" -lt "$n" ]; do
    [ "$(count_lines "$file" "$text")" -gt "$before" ] && return 0
    sleep 0.05; i=$((i + 1))
  done
  return 1
}

check_clip_guest_host() {
  if ! bench; then skip clip-guest-host "needs the guest copy bench op (paddock macvm bench copy)"; return; fi
  focus TextEdit
  paddock macvm bench copy "$app" "files guest text $$" || { fail clip-guest-host "the bench copy failed"; return; }
  i=0
  while [ "$i" -lt 6 ]; do
    [ "$(pbpaste)" = "files guest text $$" ] && { pass clip-guest-host; return; }
    sleep 0.05; i=$((i + 1))
  done
  fail clip-guest-host "pbpaste has $(pbpaste | head -c 40)"
}

check_clip_not_key() {
  focus Finder
  before=$(count_lines "$events" "the host clipboard is in the guest")
  printf 'files not key %s' "$$" | pbcopy
  sleep 1
  [ "$(count_lines "$events" "the host clipboard is in the guest")" -eq "$before" ] || { fail clip-not-key "sent while the Finder was key"; return; }
  focus TextEdit
  if wait_line_count "$events" "the host clipboard is in the guest" "$before" 0.3; then pass clip-not-key; else fail clip-not-key "not sent on key"; fi
}

check_concealed() {
  focus TextEdit
  before=$(count_lines "$events" "the host clipboard is in the guest")
  # A concealed item, as a password manager writes it.
  osascript -l JavaScript -e '
    ObjC.import("AppKit");
    const pb = $.NSPasteboard.generalPasteboard;
    pb.clearContents;
    const item = $.NSPasteboardItem.alloc.init;
    item.setStringForType("hunter2", "public.utf8-plain-text");
    item.setStringForType("", "org.nspasteboard.ConcealedType");
    pb.writeObjects($([item]));' >/dev/null 2>&1
  sleep 1
  if [ "$(count_lines "$events" "the host clipboard is in the guest")" -eq "$before" ]; then pass concealed; else fail concealed "a concealed item crossed"; fi
}

check_drop() {
  if ! bench; then skip drop "needs the synthetic drag bench op (paddock macvm bench drag) and the host windows"; return; fi
  echo "files drop" >"$work/rw/dropped-file.txt"
  paddock macvm bench drag "$app" "$work/rw/dropped-file.txt" || { fail drop "the bench drag failed"; return; }
  wait_line "$events" "dropped as open" 2 || { fail drop "no dropped as open within 2 s"; return; }
  ls -d "$profiles/$app/default/drop"/*-1/dropped-file.txt >/dev/null 2>&1 || { fail drop "no copy under drop/<time>-1/"; return; }
  pass drop
}

check_download() {
  if ! bench; then skip download "needs the guest shell bench op (paddock macvm bench sh)"; return; fi
  paddock macvm bench sh "$app" -- curl -s -o "\$HOME/Downloads/x" https://example.com/ >/dev/null 2>&1
  sleep 1
  [ -f "$profiles/$app/default/downloads/x" ] && pass download || fail download "no downloads/x on the host"
}

check_limits() {
  if ! bench; then skip limits "needs the transfer bench op"; return; fi
  for i in $(seq 1 65); do paddock macvm bench transfer "$app" 1 >/dev/null 2>&1; done
  if wait_line "$events" "more than 64 transfers in a minute" 2; then pass limits; else fail limits "the 65th transfer was not refused"; fi
}

check_configure() {
  grep -q '"downloads-link"' src/macvm/configure.rs || { fail configure "no downloads-link step in configure.rs"; return; }
  if bench; then
    paddock macvm bench configure-verify downloads-link >"$tmp/cfg.out" 2>&1 && grep -q '/Volumes/My Shared Files/downloads' "$tmp/cfg.out" && { pass configure; return; }
    fail configure "readlink ~/Downloads is not the share: $(head -2 "$tmp/cfg.out")"
  else
    skip configure "the step is in the list; the guest readlink needs paddock macvm bench configure-verify"
  fi
}

needs_session() { case $1 in shares|clip-host-guest|clip-guest-host|clip-not-key|concealed|drop|download|limits) return 0;; *) return 1;; esac; }

run_check() {
  if needs_session "$1"; then
    if ! golden_ready; then skip "$1" "no sealed golden (set PADDOCK_MACVM_GOLDEN)"; return; fi
    if [ -z "${session_up:-}" ]; then
      if start_session; then session_up=1; else fail "$1" "no hello within 90 s: $(tail -3 "$tmp/run.out" | tr '\n' ' ')"; return; fi
    fi
  fi
  case $1 in
    build) build;;
    shares) check_shares;;
    env) check_env;;
    clip-host-guest) check_clip_host_guest;;
    clip-guest-host) check_clip_guest_host;;
    clip-not-key) check_clip_not_key;;
    concealed) check_concealed;;
    drop) check_drop;;
    download) check_download;;
    limits) check_limits;;
    configure) check_configure;;
    *) echo "unknown check $1"; exit 2;;
  esac
}

if [ "$what" = all ]; then
  for c in build configure env shares clip-host-guest clip-guest-host clip-not-key concealed drop download limits; do run_check "$c"; done
else
  run_check "$what"
fi
echo "macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion)), $(sysctl -n hw.model)"
[ "$fails" -eq 0 ]
