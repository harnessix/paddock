#!/usr/bin/env bash
# The performance bench of the macOS guest, checked on a Mac.
#
# Run this script on an Apple silicon Mac. It builds the
# worktree, runs the Linux-side tests of the bench, proves the gates of
# `paddock macvm bench` (the case names, the refusal of a release build,
# the refusal of a session without --bench), runs packaging/macos/bench.sh
# with every case, and checks the JSON it wrote. Each check prints `PASS
# name`, `FAIL name: reason` or `SKIP name: reason`; the script exits 0
# when every check that ran passed.
#
# Usage:
#   packaging/macos/checks/macvm-bench.sh build    # compile, clippy, tests
#   packaging/macos/checks/macvm-bench.sh all      # build plus every check
#   packaging/macos/checks/macvm-bench.sh json     # one check by name
#
# What it needs: what packaging/macos/bench.sh needs (its header), and the
# three profiles chatgpt-desktop, textedit and quicktime. Without a sealed
# golden (PADDOCK_MACVM_GOLDEN unset) only `build`, `gates` and `env`
# run; the rest print SKIP.
#
# What proves each check:
#   build    `cargo build`, `cargo clippy --all-targets -- -D warnings` and
#            `cargo test macvm::bench` exit 0 on macOS.
#   gates    `paddock macvm bench fast` exits 1 and names the cases; a
#            release binary refuses without PADDOCK_MACVM_BENCH=1 and runs
#            with it (`bench report` on an empty folder, which only reads
#            files); a session of chatgpt-desktop started without --bench
#            makes `bench idle` exit 1 with the line that names
#            `paddock app run chatgpt-desktop --bench`.
#   bench    packaging/macos/bench.sh exits 0 with ChatGPT and TextEdit.
#   json     the JSON of that run has `model`, `macos`, `paddock` and the
#            13 measures of the 11 cases (`idle` carries idle.cpu,
#            idle.ram and idle.load); input has at least 200 samples,
#            frames at least 150, idle.cpu 60.
#   env      `PADDOCK_MACVM_BENCH_TARGET_WARM=1 packaging/macos/bench.sh
#            warm` exits non-zero and prints a MISS line that names warm.
#
# `bench.sh --baseline` writes the reference run; commit it with the
# change that it measures.
set -uo pipefail

root=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$root"
paddock="$root/target/debug/paddock"
golden=${PADDOCK_MACVM_GOLDEN:-}
status=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; status=1; }
skip() { echo "SKIP $1: $2"; }

check_build() {
  cargo build 2>&1 | tail -1 || { fail build "cargo build"; return; }
  cargo clippy --all-targets -- -D warnings >/dev/null 2>&1 || { fail build "clippy"; return; }
  cargo test macvm::bench 2>&1 | grep -q 'test result: ok' || { fail build "cargo test macvm::bench"; return; }
  pass build
}

check_gates() {
  out=$("$paddock" macvm bench fast 2>&1); code=$?
  [ "$code" -eq 1 ] && grep -q 'warm, cold' <<<"$out" || { fail gates "bench fast: exit $code: $out"; return; }
  if cargo build --release >/dev/null 2>&1; then
    rel="$root/target/release/paddock"
    out=$("$rel" macvm bench report --dir /nonexistent 2>&1); code=$?
    grep -q 'PADDOCK_MACVM_BENCH=1' <<<"$out" || { fail gates "release build ran without the variable: $out"; return; }
    PADDOCK_MACVM_BENCH=1 "$rel" macvm bench report --dir /nonexistent >/dev/null 2>&1
    [ $? -eq 1 ] || { fail gates "release build with the variable did not report"; return; }
  fi
  if [ -z "$golden" ]; then
    pass gates; return
  fi
  "$paddock" app stop chatgpt-desktop --wait 120 >/dev/null 2>&1
  "$paddock" app run chatgpt-desktop --backend macvm --network egress >/dev/null 2>&1 &
  sleep 60
  out=$("$paddock" macvm bench idle 2>&1); code=$?
  "$paddock" app stop chatgpt-desktop --wait 120 >/dev/null 2>&1
  [ "$code" -ne 0 ] && grep -q 'paddock app run chatgpt-desktop --bench' <<<"$out" \
    || { fail gates "a session without --bench was not refused: exit $code: $out"; return; }
  pass gates
}

run_json=""
check_bench() {
  [ -n "$golden" ] || { skip bench "no PADDOCK_MACVM_GOLDEN"; return; }
  out=$(packaging/macos/bench.sh --baseline 2>&1); code=$?
  echo "$out"
  run_json=$(grep -o '^bench: .*\.json$' <<<"$out" | tail -1 | sed 's/^bench: //')
  [ "$code" -eq 0 ] || { fail bench "exit $code"; return; }
  pass bench
}

check_json() {
  [ -n "$run_json" ] && [ -f "$run_json" ] || { skip json "no run JSON"; return; }
  python3 - "$run_json" <<'PY' || { fail json "see above"; return; }
import json, sys
r = json.load(open(sys.argv[1]))
for k in ("model", "macos", "paddock"):
    assert r.get(k), f"no {k}"
m = r["measures"]
want = ["warm","cold","first","quit","update","input","move","frames","idle.cpu","idle.ram","idle.load","video","disk"]
missing = [w for w in want if w not in m]
assert not missing, f"missing measures {missing}"
assert len(m) == 13, f"{len(m)} measures"
assert m["input"]["samples"] >= 200, f"input samples {m['input']['samples']}"
assert m["frames"]["samples"] >= 150, f"frames samples {m['frames']['samples']}"
assert m["idle.cpu"]["samples"] == 60, f"idle samples {m['idle.cpu']['samples']}"
print("13 measures, input", m["input"]["samples"], "frames", m["frames"]["samples"], "idle", m["idle.cpu"]["samples"])
PY
  pass json
}

check_env() {
  [ -n "$golden" ] || { skip env "no PADDOCK_MACVM_GOLDEN"; return; }
  out=$(PADDOCK_MACVM_BENCH_TARGET_WARM=1 packaging/macos/bench.sh warm 2>&1); code=$?
  [ "$code" -ne 0 ] && grep -q '^MISS warm' <<<"$out" || { fail env "exit $code: $out"; return; }
  pass env
}

what=${1:-all}
case "$what" in
  build) check_build ;;
  gates) check_gates ;;
  bench) check_bench; check_json ;;
  json) check_bench; check_json ;;
  env) check_env ;;
  all) check_build; check_gates; check_bench; check_json; check_env ;;
  *) echo "unknown check $what" >&2; exit 2 ;;
esac
exit $status
