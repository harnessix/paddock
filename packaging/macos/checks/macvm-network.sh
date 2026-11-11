#!/usr/bin/env bash
# The network exit of the macOS guest, checked on a Mac.
#
# Run this script on an Apple silicon Mac with a sealed
# golden image (PADDOCK_MACVM_GOLDEN names it, as macvm-session.sh does).
# It starts one scratch session of the profile `net` in a temporary data
# folder and runs each check below. Each check prints one line: `PASS
# name`, `FAIL name: reason` or `SKIP name: reason`. The script exits
# non-zero on a FAIL.
#
# Usage:
#   packaging/macos/checks/macvm-network.sh all
#   packaging/macos/checks/macvm-network.sh service    # one check by name
#
# Commands inside the guest run through the bench ops (`paddock macvm
# bench spawn-child -- ARGV`, `paddock macvm bench env`, both refused
# without `--bench` on the session). In a build without them, the checks
# that need a command in the guest print SKIP with that reason.
#
# What proves each check (the lines of this script and the files it reads):
#   service   `networksetup -listallnetworkservices` in the guest names one
#             service, and `-getsecurewebproxy` of it says `Server:
#             127.0.0.1`, `Port: 3128`, `Enabled: Yes`.
#   through   `curl -sS https://example.com` in the guest exits 0, and the
#             connection log has one `allow` line with `app = curl` and an
#             empty team (in a build without the connection log of the
#             firewall, the proxy's debug line in shim.log names
#             `app = curl`).
#   direct    `curl --noproxy '*' https://example.com` exits non-zero
#             within 2 s, and logs/boundary.jsonl has one `no route` line
#             whose target is `example.com` or its address (the guest
#             resolves nothing, so the line is the DNS of the gateway:
#             `udp 10.0.2.2:53` is answered REFUSED and not logged, the
#             TCP to the address never comes; the check accepts an exit
#             within 2 s with no `network` line as well, and says so).
#   syn       `nc -z -G 2 1.1.1.1 443` in the guest fails within 2 s and
#             the record of the session (`sessions/net--default.json`,
#             what `paddock app status --json` prints) shows one
#             dropped line `tcp 1.1.1.1:443`.
#   identity  with TextEdit open in the guest, a `curl` spawned as a child
#             of TextEdit (the bench op spawns it under the app) gives a
#             proxy debug line in shim.log with `parent_app =
#             com.apple.TextEdit`.
#   wake      `pmset relative wake 30` then `pmset sleepnow`; within 15 s
#             of the wake a `curl` through the proxy exits 0, and
#             events.jsonl has `sleep` and `wake` lines with the length
#             (in a build without these lines, the check reports the
#             curl alone and says so).
#   env       the bench op `env` prints the environment of the launched
#             app and `HTTPS_PROXY=http://127.0.0.1:3128` is in it.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-net.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profile="$root/profiles.noindex/net/default"
record="$root/sessions/net--default.json"
name=net

cleanup() {
  paddock app stop "$name" >/dev/null 2>&1
  sleep 2
  pgrep -f "paddock-vm app --profile $root/profiles.noindex" | xargs -r kill -9 2>/dev/null
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

build() {
  if ! cargo build --release --bin paddock --bin paddock-vm >"$tmp/build.log" 2>&1; then
    fail build "cargo build: $(tail -5 "$tmp/build.log" | tr '\n' ' ')"; return 1
  fi
  if ! codesign --force --sign - --entitlements packaging/macos/paddock-vm.entitlements "$bin/paddock-vm" >>"$tmp/build.log" 2>&1; then
    fail build "codesign of paddock-vm failed"; return 1
  fi
  if ! cargo test --release --lib -- macvm::stubnet guest::proxy guest::identity proxy:: macvm::proxy_header >"$tmp/test.log" 2>&1; then
    fail build "cargo test: $(grep -E 'FAILED|panicked' "$tmp/test.log" | head -3 | tr '\n' ' ')"; return 1
  fi
  pass build
}

# The bench op spawn-child. `guest_sh NAME CMD` runs CMD in the guest
# through `/bin/sh -c`, prints its output and returns its exit code.
# Without the op it prints SKIP for NAME and returns 99.
has_bench() { paddock macvm bench --help 2>/dev/null | grep -q spawn-child; }
guest_sh() {
  local check=$1 cmd=$2
  if ! has_bench; then
    skip "$check" "needs the bench op paddock macvm bench spawn-child"
    return 99
  fi
  paddock macvm bench spawn-child --session "$name" -- /bin/sh -c "$cmd"
}

start_session() {
  if [ -z "${PADDOCK_MACVM_GOLDEN:-}" ]; then
    skip session "PADDOCK_MACVM_GOLDEN names no sealed golden"; return 1
  fi
  mkdir -p "$root"
  cp -c -R "$PADDOCK_MACVM_GOLDEN" "$root/golden.noindex" 2>/dev/null || cp -R "$PADDOCK_MACVM_GOLDEN" "$root/golden.noindex"
  local extra=()
  has_bench && extra=(--bench)
  # --network egress: these checks are about the exit, not the panel,
  # so no question may wait for a person.
  (paddock app run "$name" --backend macvm --network egress "${extra[@]}" >"$tmp/run.log" 2>&1 &)
  for _ in $(seq 1 120); do
    grep -q '"kind": *"hello"\|"kind":"hello"' "$profile/logs/events.jsonl" 2>/dev/null && return 0
    sleep 1
  done
  fail session "no hello within 120 s: $(tail -3 "$tmp/run.log" | tr '\n' ' ')"
  return 1
}

check_service() {
  local out
  out=$(guest_sh service 'networksetup -listallnetworkservices; for s in $(networksetup -listallnetworkservices | tail -n +2 | grep -v "^\*"); do networksetup -getsecurewebproxy "$s"; done') || return
  if echo "$out" | grep -q 'Server: 127.0.0.1' && echo "$out" | grep -q 'Port: 3128' && echo "$out" | grep -q 'Enabled: Yes'; then
    pass "service ($(echo "$out" | sed -n 2p))"
  else
    fail service "$(echo "$out" | tr '\n' ' ' | cut -c1-300)"
  fi
}

check_through() {
  local out
  out=$(guest_sh through 'curl -sS -o /dev/null -w "%{http_code}" https://example.com; echo " exit $?"') || return
  if echo "$out" | grep -q 'exit 0'; then
    if grep -q 'app = curl' "$profile/logs/shim.log" 2>/dev/null; then
      pass through
    else
      fail through "curl passed but no proxy line names app = curl in shim.log"
    fi
  else
    fail through "$out"
  fi
}

check_direct() {
  local out start end
  start=$(date +%s)
  out=$(guest_sh direct "curl -sS --noproxy '*' --max-time 5 https://example.com; echo \" exit \$?\"") || return
  end=$(date +%s)
  if echo "$out" | grep -q 'exit 0'; then
    fail direct "a direct curl got through: $out"; return
  fi
  if [ $((end - start)) -gt 4 ]; then
    fail direct "the direct curl took $((end - start)) s (DNS REFUSED should fail it at once)"; return
  fi
  if grep -q '"rule":"no route' "$profile/logs/boundary.jsonl" 2>/dev/null; then
    pass "direct (boundary line: $(grep -m1 'no route' "$profile/logs/boundary.jsonl" | cut -c1-120))"
  else
    pass "direct (failed at once; no network line because the name never resolved)"
  fi
}

check_syn() {
  local out
  out=$(guest_sh syn 'nc -z -G 2 1.1.1.1 443; echo " exit $?"') || return
  if echo "$out" | grep -q 'exit 0'; then
    fail syn "a SYN to 1.1.1.1:443 got an answer"; return
  fi
  sleep 12
  if grep -q '"dst":"1.1.1.1","port":443' "$record" 2>/dev/null && grep -q '"rule":"no route' "$profile/logs/boundary.jsonl"; then
    pass syn
  else
    fail syn "no tcp 1.1.1.1:443 line in the record or the boundary log"
  fi
}

check_identity() {
  if ! has_bench; then skip identity "needs the bench op paddock macvm bench spawn-child"; return; fi
  paddock macvm bench spawn-child --session "$name" --under /System/Applications/TextEdit.app -- /usr/bin/curl -sS -o /dev/null https://example.com
  if grep -q 'parent_app = com.apple.TextEdit' "$profile/logs/shim.log" 2>/dev/null; then
    pass identity
  else
    fail identity "no proxy line with parent_app = com.apple.TextEdit in shim.log"
  fi
}

check_wake() {
  if ! has_bench; then skip wake "needs the bench op paddock macvm bench spawn-child"; return; fi
  sudo pmset relative wake 30 || { skip wake "pmset needs sudo"; return; }
  sudo pmset sleepnow
  sleep 45
  local out
  out=$(guest_sh wake 'curl -sS -o /dev/null https://example.com; echo " exit $?"') || return
  if ! echo "$out" | grep -q 'exit 0'; then
    fail wake "curl after the wake: $out"; return
  fi
  if grep -q '"kind":"wake"\|"kind": *"wake"' "$profile/logs/events.jsonl"; then
    pass wake
  else
    pass "wake (curl passed; events.jsonl has no sleep and wake lines)"
  fi
}

check_env() {
  if ! has_bench; then skip env "needs the bench op paddock macvm bench spawn-child"; return; fi
  local out
  out=$(paddock macvm bench env --session "$name")
  if echo "$out" | grep -q '^HTTPS_PROXY=http://127.0.0.1:3128$'; then
    pass env
  else
    fail env "$(echo "$out" | grep -i proxy | tr '\n' ' ')"
  fi
}

build || exit 1
if [ "$what" = build ]; then exit $fails; fi
start_session || exit $fails
case "$what" in
  all) check_service; check_through; check_direct; check_syn; check_identity; check_wake; check_env ;;
  service|through|direct|syn|identity|wake|env) "check_$what" ;;
  *) echo "unknown check $what" >&2; exit 2 ;;
esac
exit $fails
