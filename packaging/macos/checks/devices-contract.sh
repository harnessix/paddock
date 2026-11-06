#!/bin/sh
# The device-facing contract on a Mac: the contract, `paddock
# device-serve` and the out-of-process device are plain code with a parent
# pid check in place of PR_SET_PDEATHSIG and the macOS `peer_gone` of
# platform.rs. This script runs the parts that a Mac must confirm and
# prints PASS or FAIL per step. Run it from the repository root on the
# Mac:
#
#   sh packaging/macos/checks/devices-contract.sh
#
# What proves it: every line ends with PASS, and the last line is
# "devices-contract: all checks passed". Nothing here starts a harness or
# touches the user configuration: the tests use temporary folders. The
# serve tests need /usr/bin/python3 (the Xcode command line tools).
set -u
status=0
out=/tmp/devices-contract.out
step() {
  if "$@" >"$out" 2>&1; then
    echo "PASS $*"
  else
    echo "FAIL $*"; sed 's/^/    /' "$out"; status=1
  fi
}

# 1. The client, the supervisor and the out-of-process device, in-process.
#    Expected: "test result: ok" with 35 tests. The lines "supervisor
#    start to health", "one exec round trip" and "one serve round trip"
#    are the first numbers of the device bench: keep them with the
#    result of this script.
step cargo test --lib -- devices::program:: devices::supervise:: devices::outproc:: cli::device_serve:: --nocapture

# 2. The binary: a request through `paddock device-serve`, SIGTERM, and
#    the death of the parent (the parent pid check every second; the
#    supervisor and its child must be gone within 2 s). Expected: 2 tests ok.
step cargo test --test device_serve -- --nocapture

# 3. The example python script serves on a Unix socket on macOS
#    (socketserver.UnixStreamServer): `printf abc` gives ABC through the
#    binary in serve shape. Expected: the grep matches.
tmp=$(mktemp -d)
check_serve() {
  cargo run -q --bin paddock -- device example "$tmp/upper.py" >/dev/null || return 1
  cargo run -q --bin paddock -- device-serve --socket "$tmp/dev.sock" --shape serve --log "$tmp/log" -- "$tmp/upper.py" &
  sup=$!
  i=0
  while [ $i -lt 100 ]; do
    if printf 'GET /v1/health HTTP/1.1\r\nHost: paddock\r\n\r\n' | nc -U "$tmp/dev.sock" 2>/dev/null | grep -q "HTTP/1.1 200"; then break; fi
    sleep 0.1; i=$((i + 1))
  done
  printf 'POST /v1/call HTTP/1.1\r\nHost: paddock\r\nContent-Length: 3\r\n\r\nabc' | nc -U "$tmp/dev.sock" | grep -q "ABC"
  r=$?
  kill "$sup"; wait "$sup" 2>/dev/null
  return $r
}
step check_serve
rm -rf "$tmp"

if [ "$status" = 0 ]; then
  echo "devices-contract: all checks passed"
else
  echo "devices-contract: a check failed (see FAIL lines above)"
fi
exit "$status"
