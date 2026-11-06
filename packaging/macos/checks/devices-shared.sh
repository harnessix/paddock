#!/bin/sh
# Shared devices: `scope = "shared"`, one device instance per user.
#
# Steps 1 to 4 run on Linux and on a Mac. Steps 5 to 8 need a Mac: they
# check the LaunchAgent with a `Sockets` entry, the socket that launchd
# keeps when the instance exits on idle, and the uninstall. Step 9 needs a
# Linux host with a systemd user session, bubblewrap and FUSE; the tests
# skip where they are missing.
#
# Run it from the repository root:
#
#   sh packaging/macos/checks/devices-shared.sh
#
# What proves it: every line ends with PASS and the last line is
# "devices-shared: all checks passed". A step that prints "skipped" is not
# a pass. Nothing here starts a harness or touches your own user
# configuration: the tests work in temporary folders under
# ~/.cache/paddock-tests, and the Mac steps use a temporary HOME.
set -u
status=0
out=/tmp/devices-shared.out
step() {
  if "$@" >"$out" 2>&1 && ! grep -q '^skipped' "$out"; then
    echo "PASS $*"
  else
    echo "FAIL $*"; sed 's/^/    /' "$out" | tail -n 40; status=1
  fi
}

# 1. The pieces, platform-neutral: the socket path and the ListenStream of
#    the unit, the table hash that catches a reconfiguration, the start
#    kinds, the start lock with ten takers, the unit and the property list
#    text, and the refusals. Expected: 14 shared tests pass.
step cargo test --lib -- devices::shared:: --nocapture

# 2. The units and the LaunchAgent of the gateway executor still have the
#    shape they had before the writers moved to src/units.rs, and a unit
#    of an older paddock still counts as paddock's own, so an upgrade can
#    still remove it. Expected: the units tests and the activation tests
#    pass.
step cargo test --lib -- units:: gatewayd::activation:: --nocapture

# 3. The instance end to end, with real processes: two runs share one
#    instance and one pid and both get an answer with their own
#    PADDOCK_CALLER; the user count goes 1, 2, 1, 0; the instance exits
#    after idle (2 s in the test); a broker killed with -9 drops its lease
#    within 2 s; ten brokers that start at once get one instance; a table
#    change gives `reconfigured` and `paddock device restart` clears it;
#    without a user manager the first broker owns it, the notice names the
#    run, the instance ends with that run and the next request of another
#    run starts a new one; `paddock device stop` ends it now; `ls`, `show`
#    and `ps` report the shared lifetime. Expected: 9 tests pass. The two
#    steps that need a systemd user manager or bubblewrap print "skipped"
#    when the host has none, and step 9 below runs them.
step cargo test --test devices_shared -- --nocapture

# 4. The whole suite, as CI runs it. Expected: "test result: ok" for every
#    binary. In a paddock sandbox the bwrap escape probe and the two
#    gateway "approved ... runs in the executor" tests fail; on a host
#    they pass.
step cargo test --no-fail-fast

# The Mac steps. Run them on the Mac, in this order, from the repository
# root, with an installed paddock on the PATH: `paddock device install`
# refuses a build in a target folder, because paddock never points a unit
# or a LaunchAgent at one ("this paddock is .../target/debug/paddock, a
# build in a target folder"). So run `bin/install` first and use `paddock`,
# not ./target/release/paddock. Each step says what proves it.
mac_note() {
  cat <<'TXT'

The Mac steps (run them by hand on the Mac, in order):

 5. The LaunchAgent with a Sockets entry.
      export MACHOME=$(mktemp -d)
      export XDG_CONFIG_HOME=$MACHOME/cfg XDG_STATE_HOME=$MACHOME/state
      mkdir -p "$XDG_CONFIG_HOME/paddock"
      cat > "$XDG_CONFIG_HOME/paddock/config.toml" <<'EOF'
      [devices.up]
      shape = "exec"
      enabled = true
      mode = "allow"
      grant = ["all"]
      scope = "shared"
      idle = 20
      command = ["/usr/bin/tr", "a-z", "A-Z"]
      EOF
      paddock device install up
    Expected: the output names
    ~/Library/LaunchAgents/com.harnessix.paddock.device.up.plist and says
    "launchd owns the socket and starts the shared instance on the first
    request". Then:
      plutil -lint ~/Library/LaunchAgents/com.harnessix.paddock.device.up.plist
      /usr/libexec/PlistBuddy -c 'Print :Sockets:Listeners:SockPathName' \
        ~/Library/LaunchAgents/com.harnessix.paddock.device.up.plist
      launchctl print "gui/$(id -u)/com.harnessix.paddock.device.up" | head -n 30
    Expected: plutil says OK; PlistBuddy prints the socket path, which is
    $XDG_STATE_HOME/paddock/devices/up.sock; launchctl print shows the job with
    "sockets = { Listeners => ... }" and no running pid yet.

 6. The first request from a Seatbelt run starts the instance through the
    socket.
      paddock device check up --send hello
    Expected: "shared instance ready in N ms
    (launch agent)" and the response HELLO. Then, from a real Seatbelt
    run:
      paddock run claude --backend seatbelt --network none
      # in the run:  printf hello | paddock-host device up -
      # on the Mac:  paddock ps
    Expected: the run prints HELLO, `paddock ps` shows one line "shared
    device up   1 user, running 0m, pid N, unit
    com.harnessix.paddock.device.up", and
    paddock device ls shows "shared, 1 user".

 7. Idle exit, and launchd keeps the socket.
      # end the run of step 6, then wait for idle (20 s) and look:
      launchctl print "gui/$(id -u)/com.harnessix.paddock.device.up" | grep -E 'state|pid'
      ls -l@ "$(/usr/libexec/PlistBuddy -c 'Print :Sockets:Listeners:SockPathName' \
        ~/Library/LaunchAgents/com.harnessix.paddock.device.up.plist)"
      ls "$XDG_STATE_HOME/paddock/devices/instances/up/" 2>&1
      cat "$XDG_STATE_HOME/paddock/devices/log/up/shared.log" | tail -n 5
    Expected: launchctl shows the job loaded with no pid
    (state "waiting for initial demand" or no "pid = " line); the socket
    file is still there and owned by you with mode srw-------; the
    instances folder is gone or empty; the log ends with "idle for 20 s
    with no user: exit". Then one more request starts it again:
      paddock device check up --send again
    Expected: AGAIN, and a new pid.

 8. The uninstall removes the plist and the job.
      paddock device uninstall up
      ls ~/Library/LaunchAgents/com.harnessix.paddock.device.up.plist 2>&1
      launchctl print "gui/$(id -u)/com.harnessix.paddock.device.up" 2>&1 | head -n 3
      paddock device show up | grep -A1 'start '
    Expected: the plist is gone ("No such file or
    directory"), launchctl print fails with "Could not find service", and
    `device show` says "the first request starts the shared instance
    (owned by the run)". A plist that paddock did not write is left alone:
      printf '<plist/>\n' > ~/Library/LaunchAgents/com.harnessix.paddock.device.up.plist
      paddock device uninstall up
    Expected: "nothing to remove", and the file is still there. Remove it
    yourself afterwards, and remove $MACHOME.

 9. On a Linux host with a systemd user session, bubblewrap and fuse3
    (these two tests skip without them):
      cargo test --test devices_shared -- --nocapture \
        with_a_systemd_user_manager an_agent_sandbox
    Expected: both tests pass with no "skipped" line. The first one uses
    the real user manager and the real XDG_RUNTIME_DIR, with a device name
    of its own (sdPIDxHEX), and removes its units at the end, also after a
    failed assert. It proves that the transient unit
    paddock-device-NAME.service holds the instance while it runs, that
    `systemctl --user stop` ends it and the next request starts a new one,
    and that `paddock device install` writes paddock-device-NAME.socket,
    links it into the user manager, and that the first connection starts
    the service with no systemd-run. Afterwards
      systemctl --user list-units --all 'paddock-device-sd*'
    lists nothing. The second proves that two
    agent sandboxes drive one shared instance over /dev/paddock, that the
    user count is 2, and that the instance exits after both runs ended.
TXT
}

if [ "$status" = 0 ]; then
  echo "devices-shared: all checks passed"
else
  echo "devices-shared: a check failed"
fi
mac_note
exit "$status"
