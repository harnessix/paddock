#!/usr/bin/env bash
# The quiet steps of the golden (src/macvm/configure_quiet.rs) and the
# connections of the guest's own services, measured on a Mac.
#
# Run this script on an Apple silicon Mac with a sealed
# golden image (PADDOCK_MACVM_GOLDEN names it, as macvm-network.sh does),
# with nothing else open. No firewall question comes up: the session runs
# with --network none, and nobody answers anything. Each check prints one
# line: `PASS name`, `FAIL name: reason` or `SKIP name: reason`. The
# script exits non-zero on a FAIL.
#
# Usage:
#   packaging/macos/checks/macvm-quiet.sh all
#   packaging/macos/checks/macvm-quiet.sh steps     # one check by name
#
# What proves each check:
#   steps    the event log of the golden (golden.noindex/events.jsonl) has
#            one `configure step quiet-NAME: ok` line for each quiet step.
#            A line with FAILED names a setting that does not take on this
#            macOS: the golden is fine, the firewall denies the service
#            anyway, and the script prints the line. The settings:
#              quiet-widgets-hidden    Desktop & Dock, Show Widgets (com.apple.WindowManager StandardHideWidgets)
#              quiet-widgets-ignored   pluginkit -e ignore of the Weather, Stocks and News widgets
#              quiet-siri-off          Apple Intelligence & Siri (com.apple.assistant.support Assistant Enabled)
#              quiet-suggestions-off   Spotlight and Look Up suggestions (com.apple.lookup.shared LookupSuggestionsDisabled)
#              quiet-agents-off        launchctl disable of newsd, weatherd, assistantd, Siri.agent, parsec-fbf, suggestd
#              quiet-icloud-setup-seen the Apple Account step of the setup counts as seen (com.apple.SetupAssistant)
#              quiet-ads-off           Apple Advertising, Personalized Ads (com.apple.AdLib)
#              quiet-no-macos-auto-install  Software Update, Install macOS updates (AutomaticallyInstallMacOSUpdates)
#              quiet-location-off      Location Services (com.apple.locationd LocationServicesEnabled)
#              quiet-analytics-off     Analytics & Improvements (DiagnosticMessagesHistory AutoSubmit)
#            The two steps that were there before, no-software-update-check
#            and no-software-update-download, print `ok` too.
#   traffic  a scratch session `quiet` with no app and --network none,
#            idle for PADDOCK_QUIET_SECS seconds (default 180): the
#            connection log names no weather-edge, stocks-edge or
#            news-edge host. The line lists every host that the guest
#            tried, with its count and process.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-quiet.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
name=quiet
profile="$root/profiles.noindex/$name/default"
log="$profile/logs/connections.jsonl"

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
  if ! cargo test --release --lib -- macvm::configure_quiet macvm::configure >"$tmp/test.log" 2>&1; then
    fail build "cargo test: $(grep -E 'FAILED|panicked' "$tmp/test.log" | head -3 | tr '\n' ' ')"; return 1
  fi
  pass build
}

check_steps() {
  local events="${PADDOCK_MACVM_GOLDEN:-}/events.jsonl" step line
  [ -n "${PADDOCK_MACVM_GOLDEN:-}" ] && [ -f "$events" ] || { skip steps "PADDOCK_MACVM_GOLDEN names no golden with an events.jsonl"; return; }
  for step in quiet-widgets-hidden quiet-widgets-ignored quiet-siri-off quiet-suggestions-off \
    quiet-agents-off quiet-icloud-setup-seen quiet-ads-off quiet-no-macos-auto-install \
    quiet-location-off quiet-analytics-off no-software-update-check no-software-update-download; do
    line=$(grep -o "configure step $step: [^\"]*" "$events" | tail -1)
    case "$line" in
      "configure step $step: ok"*) pass "steps $step" ;;
      "") fail "steps $step" "no line in $events: the setup of this golden is older, or the step did not run" ;;
      *) fail "steps $step" "$line" ;;
    esac
  done
}

check_traffic() {
  local secs=${PADDOCK_QUIET_SECS:-180} report
  [ -n "${PADDOCK_MACVM_GOLDEN:-}" ] || { skip traffic "PADDOCK_MACVM_GOLDEN names no sealed golden"; return; }
  mkdir -p "$root"
  cp -c -R "$PADDOCK_MACVM_GOLDEN" "$root/golden.noindex" 2>/dev/null || cp -R "$PADDOCK_MACVM_GOLDEN" "$root/golden.noindex"
  (paddock app run "$name" --backend macvm --network none >"$tmp/run.log" 2>&1 &)
  for _ in $(seq 1 120); do
    grep -q '"kind": *"hello"\|"kind":"hello"' "$profile/logs/events.jsonl" 2>/dev/null && break
    sleep 1
  done
  grep -q '"kind": *"hello"\|"kind":"hello"' "$profile/logs/events.jsonl" 2>/dev/null \
    || { fail traffic "no hello within 120 s: $(tail -3 "$tmp/run.log" | tr '\n' ' ')"; return; }
  sleep "$secs"
  report=$(python3 - "$log" <<'PY'
import json, sys
hosts = {}
try:
    lines = open(sys.argv[1]).read().splitlines()
except OSError:
    lines = []
for line in lines:
    try:
        e = json.loads(line)
    except ValueError:
        continue
    k = (e.get("host", "?"), e.get("app", "?"))
    hosts[k] = hosts.get(k, 0) + int(e.get("count", 1))
loud = sorted(h for h, _ in hosts if any(w in h for w in ("weather-edge", "stocks-edge", "news-edge")))
print("LOUD " + " ".join(loud) if loud else "QUIET")
for (h, a), n in sorted(hosts.items(), key=lambda x: -x[1]):
    print("  %s x%d (%s)" % (h, n, a))
PY
)
  case "$report" in
    QUIET*) pass "traffic (${secs} s idle):"; printf '%s\n' "$report" | tail -n +2 ;;
    *) fail traffic "the widgets still connect after ${secs} s: $(printf '%s' "$report" | head -1)"; printf '%s\n' "$report" | tail -n +2 ;;
  esac
}

build || exit 1
case "$what" in
  build) ;;
  all) check_steps; check_traffic ;;
  steps|traffic) "check_$what" ;;
  *) echo "unknown check $what" >&2; exit 2 ;;
esac
exit $fails
