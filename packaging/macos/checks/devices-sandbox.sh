#!/bin/sh
# The device sandbox on Linux. Nothing of this part runs on a Mac (the
# Seatbelt device sandbox has its own script, devices-seatbelt.sh). This
# script is for a Linux host with bubblewrap, /dev/fuse and a systemd
# user instance; the tests that need them skip without them. Run it from the
# repository root on a Linux host with bwrap, fuse3 and a systemd user
# session:
#
#   sh packaging/macos/checks/devices-sandbox.sh
#
# What proves it: every line ends with PASS, and the last line is
# "devices-sandbox: all checks passed". Each step prints "skipped" lines
# when a need is missing; a skipped step is not a pass. Nothing here starts
# a harness or touches the user configuration: the tests use temporary
# folders under ~/.cache/paddock-tests. On a Mac, only step 1 runs; the
# others print FAIL with "skipped" in their output.
set -u
status=0
out=/tmp/devices-sandbox.out
step() {
  if "$@" >"$out" 2>&1 && ! grep -q '^skipped:' "$out"; then
    echo "PASS $*"
  else
    echo "FAIL $*"; sed 's/^/    /' "$out" | tail -n 40; status=1
  fi
}

# 1. The spec, the refusals, the lifetime through the thread launcher and
#    the three command.rs tests of 0.2.1 through the sandbox source.
#    Expected: "test result: ok" with 9 sandbox tests and 12 outproc tests.
step cargo test --lib -- devices::sandbox:: devices::outproc:: --nocapture

# 2. Real bwrap, no FUSE needed: the view of a device program (exec and
#    serve give ABC; caller project:ro reads and gets EROFS on a write; ro
#    file and an empty hidden home; no network without connect; connect
#    reaches a loopback server of the test and the audit line has the
#    bytes and lan; a deliver as = "file" secret reads once per open), the
#    ssh agent (the key is listed, the key file is absent, ssh-add -D is
#    refused), kill -9 of the paddock process ends the instance within 2 s,
#    clear_stale, logs, and the start p50 of ten starts. Expected: the
#    tests pass and the output has "device sandbox start (bwrap, exec,
#    health answered): p50 N ms, p95 M ms" (target p50 under 250 ms).
step cargo test --test devices_sandbox -- --nocapture the_view an_ssh_agent an_instance_dies

# 3. With FUSE: an agent sandbox drives the instance (paddock-host device
#    up - gives ABC), paddock ps shows the device row and --json has it,
#    paddock device ls shows the instance, stop ends it and the next
#    request starts a new one, restart gives a new pid, the instances end
#    with the run within 2 s, stats counts the three requests; two runs
#    get two instances and kill -9 of one paddock leaves none of its own.
step cargo test --test devices_sandbox -- --nocapture an_agent_sandbox two_runs

# 4. With a systemd user instance and the session bus: paddock freeze ID
#    freezes the device scope paddock-dev-<id>-ws.scope (cgroup.freeze is
#    1) and thaw thaws it.
step cargo test --test devices_sandbox -- --nocapture freeze_reaches

# 5. The whole suite, as CI runs it.
step cargo test --no-fail-fast

if [ "$status" = 0 ]; then
  echo "devices-sandbox: all checks passed"
else
  echo "devices-sandbox: a check failed"
fi
exit "$status"
