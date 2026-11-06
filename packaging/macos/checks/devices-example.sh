#!/bin/sh
# The worked example `mac`, end to end. The tests of the example that
# need a real device sandbox skip on a host without bubblewrap and FUSE. Two
# parts: part A runs on a Linux host with bubblewrap and FUSE, part B on
# a Mac. Run it from the repository root:
#
#   sh packaging/macos/checks/devices-example.sh linux    # part A
#   sh packaging/macos/checks/devices-example.sh mac      # part B
#
# What proves it: every line ends with PASS, and the last line is
# "devices-example: all checks passed". A step that prints "skipped" is
# not a pass. Part A touches only temporary folders under
# ~/.cache/paddock-tests. Part B writes the example into your real
# ~/.config/paddock/config.toml and a script under ~/.config/paddock/devices;
# the last step removes them again.
set -u
part=${1:-linux}
status=0
out=/tmp/devices-example.out
step() {
  if "$@" >"$out" 2>&1 && ! grep -q 'skipped:' "$out"; then
    echo "PASS $*"
  else
    echo "FAIL $*"; sed 's/^/    /' "$out" | tail -n 60; status=1
  fi
}
manual() {
  echo "CHECK $1"
  echo "      $2"
}

if [ "$part" = linux ]; then
  # A1. The example tables parse to the expected ones, the script, the steps, the
  #     keyscan line, `add --example mac` writes and refuses twice, the
  #     README blocks parse and no sentence is over 25 words.
  #     Expected: "test result: ok" three times.
  step cargo test --lib -- devices::examples cli::device devices::escape
  step cargo test --test readme_devices

  # A2. The stand-in Mac (an sshd of your user on the loopback): the three
  #     layers of tests/devices_mac_example.rs. The first needs no
  #     sandbox; the other two need bubblewrap and FUSE:
  #     two workers in the pasture `workers` build their worktree through
  #     `printf check | paddock-host device mac -` (CHECK_EXIT 0, two cargo
  #     lines), a wrong request exits 8 with "the device rejected the
  #     request", the agent sandbox has no agent socket and no ssh files,
  #     each worker gets its own instance and its own paddock/<caller> on
  #     the stand-in, the audit has the device line with "allowed by grant
  #     pasture:workers", the connect lines and the credential lines and
  #     never a body, and `paddock doctor --escape --device mac` passes
  #     every check (key file absent, loopback and LAN unreachable, the
  #     connect port reachable, ~/.ssh shows only known_hosts_mac). The
  #     third test does the same with a TCP echo and a copy script, with
  #     no sshd. Needs: sshd, ssh, ssh-keygen, ssh-agent, ssh-add, rsync.
  #     Expected: "test result: ok. 3 passed" and no "skipped:" line.
  step cargo test --test devices_mac_example -- --nocapture

  # A3. The bench, release, both layers: the contract path and the full
  #     path through bubblewrap and FUSE. A "NO" in the met column is not a failure of this
  #     script: a number below target goes into the note with its
  #     reason.
  #     Expected: two tables, "test result: ok. 2 passed".
  step cargo test --release --test devices_bench -- --ignored --nocapture
  cat "$out" | grep -A 20 "devices bench" || true

  # A4. The whole suite, as CI runs it on a Linux host with bubblewrap,
  #     FUSE and a systemd user instance. Nothing may skip but the tests
  #     that name a Mac.
  step cargo test --no-fail-fast
fi

if [ "$part" = mac ]; then
  # B0. Plain code on macOS: the example module, the CLI, the probe
  #     table, the README blocks.
  step cargo test --lib -- devices::examples cli::device devices::escape
  step cargo test --test readme_devices

  # B1. The real example from this Mac to itself:
  #     a Seatbelt agent run and a Seatbelt device sandbox, the device
  #     reaches 127.0.0.1:2222 to HOST:22 of this Mac. Remote Login must
  #     be on. Replace HOST with the Bonjour name of this Mac (`hostname`)
  #     and USER with your account. The steps the command prints are the
  #     steps to do once, in order; step 3 shows the keyscan line.
  echo "--- B1: the example on this Mac"
  echo "    paddock device add --example mac --host HOST --user USER"
  manual "the command printed the tables, the script path (mode 0755: ls -l ~/.config/paddock/devices/mac/build.sh) and six steps naming HOST and USER" \
         "do steps 1 to 5 as printed (ssh-keygen, ssh-add, ssh-copy-id, ssh-keyscan >> ~/.ssh/known_hosts_mac, the pastures line)"
  echo "    cd ~/src/app && paddock device check mac --send check"
  manual "the output has 'started in N ms (health answered)', a 'request:' line with http 200 and the cargo output" \
         "a failed start names 'paddock device logs mac --sandbox ...'"
  echo "    paddock device show mac"
  manual "the summary names the credential mac-ssh, 'connect: 127.0.0.1:2222 inside reaches HOST:22', and the caller project (the card of the first request of a worker shows the same three lines)" ""
  echo "    paddock codex --pasture workers   # in ~/src/app, then in the sandbox:"
  echo "    printf test | paddock-host device mac -"
  manual "the card shows 'allow the device mac for this sandbox' with the key name, the connect target and the caller mount; after 'allow for this session' the cargo output streams line by line and the exit code is 0 (7 after a failed test run, 8 for 'printf x | ...')" \
         "the audit line (~/.local/state/paddock/host-audit.jsonl) of the device-connect capability says \"lan\": true"
  # B2. Cancel: close the terminal of the codex run during a build.
  echo "--- B2: cancel"
  echo "    printf test | paddock-host device mac -   # then close the terminal window while cargo runs"
  manual "on this Mac, 'last | head' or 'log show --predicate \"process == \\\"sshd\\\"\" --last 2m' shows the session closed within 2 s of the close; 'pgrep -fl cargo' finds no build of paddock/<caller> a few seconds later" ""
  # B3. The real example from a Linux host to this Mac: the same
  #     commands on the Linux host, with --host the name of this Mac.
  echo "--- B3: from a Linux host"
  manual "on the Linux host: paddock device add --example mac --host <this mac> --user USER, the steps, paddock device check mac --send check, then a worker's 'printf test | paddock-host device mac -' streams the cargo output" ""
  # B4. The escape probe in the Seatbelt device sandbox of the example
  #     (after B1 added it): the key file absent, the real home hidden,
  #     the host loopback unreachable, the connect port reachable, no
  #     paddock-host, the caller mount read-only. The LAN and the
  #     interface checks skip on macOS (no /proc/net/route, no
  #     /sys/class/net). Expected: "every check passed".
  step sh -c 'cd ~/src/app && paddock doctor --escape --device mac 2>&1 | tee /dev/stderr | grep -q "every check passed"'
  # B5. Remove the example again.
  echo "--- B5: cleanup"
  echo "    remove the three tables and the [pastures.workers] line from ~/.config/paddock/config.toml, then: rm -r ~/.config/paddock/devices/mac ~/.ssh/known_hosts_mac"
fi

if [ "$status" = 0 ]; then
  echo "devices-example: all automatic checks passed (the CHECK lines are yours)"
else
  echo "devices-example: a check failed"
fi
exit "$status"
