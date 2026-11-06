#!/bin/sh
# The device sandbox on macOS (Seatbelt). Only a Mac runs
# sandbox-exec. Run it from the repository root on a Mac with
# the Xcode command line tools (python3, ssh-agent, nc):
#
#   sh packaging/macos/checks/devices-seatbelt.sh
#
# What proves it: every line ends with PASS, and the last line is
# "devices-seatbelt: all checks passed". A step that prints "skipped" is
# not a pass. Nothing here starts a harness or touches the user
# configuration: the tests use temporary folders under
# ~/.cache/paddock-tests and sockets under /tmp/paddock-<uid>. The real
# home of the user is the denied home of the device sandbox: no test
# reads or writes anything in it.
set -u
status=0
out=/tmp/devices-seatbelt.out
step() {
  if "$@" >"$out" 2>&1 && ! grep -q 'skipped:' "$out"; then
    echo "PASS $*"
  else
    echo "FAIL $*"; sed 's/^/    /' "$out" | tail -n 40; status=1
  fi
}

# 1. Plain code: the rendered profile (the exact rules in depth order,
#    localhost only with connect, the home denied), the environment, the
#    launch line, the launcher of the host (Seatbelt on macOS) and the
#    state file field. Expected: "test result: ok" with 9 tests.
step cargo test --lib -- backend::seatbelt::device::tests

# 2. sandbox-exec with the rendered profile and environment, no
#    supervisor: the program reads ro, writes rw, reads the caller's
#    project and gets "Operation not permitted" on a write, cannot read
#    the real home or the instance folder, reads the secret FIFO and
#    cannot remove it; no TCP without connect, the connect port on
#    127.0.0.1 and nothing else with it, and a bind on localhost inside;
#    the ssh agent socket and the connect socket connectable, the key
#    file denied, the agent socket not removable; the supervisor can
#    create dev.sock; sandbox-exec accepts the profile of every shape of
#    table (if "sandbox_exec_accepts" or "the_supervisor_can_create"
#    fails with a profile error, the fallback is a connect relay on the
#    host).
#    Expected: "test result: ok" with 5 tests.
step cargo test --lib -- backend::seatbelt::device::macos_tests --nocapture

# 3. End to end through `paddock device check`: exec and serve give
#    ABC; caller project:ro at the host path; ro file, empty hidden home,
#    the real ~/.ssh and history hidden, TERM=dumb, the shared /tmp
#    denied; no network without connect; connect reaches a loopback
#    server and the audit line has the bytes; a secret FIFO at a host
#    path; the ssh agent (the key is listed, the key file is denied,
#    ssh-add -D is refused); ten starts with the p50 and p95 printed;
#    logs; doctor names the ignored mem and cpus; kill -9 of the paddock process ends the instance within 2 s
#    (the parent pid check), and the next start clears the stale state
#    and socket folders.
step cargo test --test seatbelt_devices -- --nocapture the_view an_ssh_agent an_instance_dies

# 4. A Seatbelt agent run drives the instance with `paddock-host device
#    up -` (ABC), the catalog says run = sandbox and platform = macos,
#    paddock ps shows the device row and --json has it, paddock device ls
#    shows the instance, stop ends it and the next request starts a new
#    one, restart gives a new pid, logs read while it runs, the instances
#    end with the run within 2 s, stats counts the three requests.
step cargo test --test seatbelt_devices -- --nocapture a_seatbelt_run

# 5. The whole suite, as CI runs it on a Mac.
step cargo test --no-fail-fast

# Not here: a VM run with a device. paddock does not route a VM run's
# devices to the guest yet, so `paddock-host device upper x` in
# the guest cannot be checked yet; the device sandbox of such a run
# is the one of step 3 and 4, on the Mac.

if [ "$status" = 0 ]; then
  echo "devices-seatbelt: all checks passed"
else
  echo "devices-seatbelt: a check failed"
fi
exit "$status"
