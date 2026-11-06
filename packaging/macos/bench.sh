#!/usr/bin/env bash
# The performance bench of the macOS guest.
#
# Run this script on an Apple silicon Mac with a sealed
# golden and the profiles `chatgpt-desktop`, `textedit` and `quicktime`
# (`paddock app install --drop`, then drop the three apps on `Paddock
# Apps`). It runs every case through `paddock macvm bench CASE --json`,
# writes `.paddock/bench/<date>.json`, prints the table, and exits
# non-zero with the case named when a target of
# packaging/macos/bench/targets.json is missed.
#
# Usage:
#   packaging/macos/bench.sh                 # every case
#   packaging/macos/bench.sh warm idle       # these cases, then the report
#   packaging/macos/bench.sh --baseline      # every case, and the run
#                                            # becomes packaging/macos/bench/baseline.json
#
# What it needs: the Xcode command line tools (`swift` runs the key and
# the frames helper), Accessibility and Screen Recording granted to the
# terminal that runs it (the grants go to the script, not to paddock), a
# 1080p video in PADDOCK_MACVM_BENCH_VIDEO for `video`, and a development
# build (`cargo build`) or PADDOCK_MACVM_BENCH=1 for a release build.
# PADDOCK_MACVM_BENCH_TARGET_<MEASURE>=N lowers a target for a test of
# the exit code: `PADDOCK_MACVM_BENCH_TARGET_WARM=1 packaging/macos/bench.sh
# warm` exits 1 and names `warm`.
#
# The cases: warm, cold, first, quit and update from the event log, five
# runs each; input from 200 keys through the host window and the
# `ax.value` events; move from the shim's own clock over injected
# `windows` events; frames from a ScreenCaptureKit capture of the host
# window while the guest runs a 60 Hz counter in TextEdit; idle and video
# from `ps` of the com.apple.Virtualization.VirtualMachine process (the
# guest memory lives there, not in paddock) and the guest load; disk from
# `du` of the profile.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
paddock=${PADDOCK_BIN:-"$root/target/debug/paddock"}
if [ ! -x "$paddock" ]; then
  echo "bench: no $paddock: run cargo build first, or set PADDOCK_BIN" >&2
  exit 2
fi
export PADDOCK_MACVM_BENCH_TOOLS="$root/packaging/macos/bench"
date=$(date -u +%Y-%m-%dT%H%M%SZ)
out="$root/.paddock/bench/$date.json"
cases_dir="$root/.paddock/bench/cases-$date"
mkdir -p "$cases_dir"

baseline=0
cases=()
for a in "$@"; do
  case "$a" in
    --baseline) baseline=1 ;;
    -h | --help) sed -n '2,40p' "$0"; exit 0 ;;
    *) cases+=("$a") ;;
  esac
done
if [ ${#cases[@]} -eq 0 ]; then
  cases=(warm cold first quit update input move frames idle video disk)
fi

echo "bench: macOS $(sw_vers -productVersion) on $(sysctl -n hw.model), paddock $("$paddock" --version 2>/dev/null | head -1)"
failed=()
for c in "${cases[@]}"; do
  echo "bench: $c"
  if ! "$paddock" macvm bench "$c" --dir "$cases_dir"; then
    echo "bench: $c did not run" >&2
    failed+=("$c")
  fi
done

# Every case of the run into the table and the JSON; exit 1 names the
# missed measures.
status=0
"$paddock" macvm bench report --dir "$cases_dir" --out "$out" || status=$?
echo "bench: $out"
if [ ${#failed[@]} -gt 0 ]; then
  echo "bench: cases that did not run: ${failed[*]}" >&2
  status=1
fi
if [ "$baseline" -eq 1 ] && [ "$status" -eq 0 ]; then
  cp "$out" "$root/packaging/macos/bench/baseline.json"
  echo "bench: packaging/macos/bench/baseline.json is this run; commit it"
fi
exit "$status"
