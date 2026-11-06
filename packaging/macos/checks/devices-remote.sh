#!/bin/sh
# Remote devices on a Mac: `paddock device serve-remote` on the Mac,
# reached from a Linux host over `ssh://mac`, serves a `run = "sandbox"`
# device: the build runs in a Seatbelt device sandbox on the Mac, the
# output streams to the Linux agent, and the card on the Mac names the
# Linux host and the sandbox. The code is platform-neutral; what only a
# Mac can show is the Seatbelt device sandbox behind serve-remote and the
# card there. Run it on the Mac from the repository root:
#
#   sh packaging/macos/checks/devices-remote.sh
#
# What proves it: every line ends with PASS, and the last line is
# "devices-remote: all checks passed". Steps 1 and 2 use temporary
# folders and never touch the user configuration. Step 3 needs a Linux
# host that reaches this Mac over ssh with a key (no prompt): it adds a
# device table to the user configuration of the Mac, prints the lines for
# the Linux side, and removes the table after. It asks before it edits
# the configuration.
set -u
status=0
out=/tmp/devices-remote.out
step() {
  if "$@" >"$out" 2>&1; then
    echo "PASS $*"
  else
    echo "FAIL $*"; sed 's/^/    /' "$out"; status=1
  fi
}

# 1. The forward, the serving side and the grant without a Mac
#    anything: two paddock homes in one process, the fake ssh, the
#    errno mapping, the stream, the cancel within 500 ms, the session
#    file, the texts. Expected: "test result: ok".
step cargo test --lib -- devices::forward:: devices::grant:: devices::config:: devices::socket::

# 2. The paddock binary as the other side: a fake ssh starts
#    `paddock device serve-remote` with SSH_CONNECTION as sshd would, the
#    socket mode takes many connections and ends on SIGTERM, `check`,
#    `ls`, `add` and `--forget`. The device sandbox runs with the thread
#    launcher here, so this proves the transport, not Seatbelt. Expected:
#    3 tests ok. The line "first remote request over the fake ssh" is
#    the round trip on the Mac.
step env PADDOCK_TEST_DEVICE_LAUNCHER=thread cargo test --test devices_remote -- --nocapture

# 3. The real thing: a Seatbelt device sandbox on this Mac behind
#    serve-remote, reached from a Linux host over ssh. This step is
#    interactive.
cfg="$HOME/.config/paddock/config.toml"
printf 'Step 3 adds [devices.mac-build] to %s for the length of the check and removes it after. Continue? [y/N] ' "$cfg"
read -r yes
if [ "$yes" = y ]; then
  printf 'The Linux host that will call this Mac (its address or a name that resolves to it, as ssh on it sees this Mac): '
  read -r linux
  printf 'The ssh destination of this Mac from that Linux host (for example me@mac.local): '
  read -r dest
  tmp=$(mktemp -d)
  cat >"$tmp/mac-build.sh" <<'SH'
#!/bin/sh
# A build on the Mac: three lines, one per second, so the stream shows.
echo "uname=$(uname -s)"
echo "home=$HOME"
echo "caller=$PADDOCK_CALLER"
for n in 1 2 3; do echo "step $n"; sleep 1; done
cat
SH
  chmod 755 "$tmp/mac-build.sh"
  cp "$cfg" "$tmp/config.toml.bak" 2>/dev/null || true
  step cargo run -q --bin paddock -- device add mac-build --exec "$tmp/mac-build.sh" --grant "remote:$linux" --stream
  # The device sandbox of the Mac starts and answers on its own.
  step cargo run -q --bin paddock -- device check mac-build
  cat <<TEXT
    On the Linux host, make sure that 'ssh $dest paddock --version' prints a
    version without a prompt (paddock on the PATH of a non-interactive shell:
    for zsh, put the Homebrew path into ~/.zshenv on this Mac). Then add the
    forward there and start a sandbox with it:
      paddock device add mac-build --run remote --target ssh://$dest --device build --stream
      paddock device check mac-build
      paddock shell claude --device mac-build
    Expected from check: "mac-build: reached ssh://$dest in N ms" and a line
    "device    build there: state on, shape exec, run sandbox, platform macos".
    In the sandbox on Linux, type:
      printf hello > /dev/paddock/mac-build; cat /dev/paddock/mac-build
    (or: printf hello | paddock-host device mac-build -)
    Expected on Linux: a card "allow the device \`mac-build\` for this sandbox: it
    connects to $dest with your ssh as you". Answer "Allow for this session".
    Expected on this Mac: a card "allow the device \`build\` for a remote caller
    from $linux (sandbox ID on HOSTNAME)" with the sandbox id of the Linux run
    and the Linux host name. Answer "Allow for this session".
    Expected output on Linux, one line per second: uname=Darwin, home=/Users/...
    (the device sandbox home, not your home), caller=remote:$linux, step 1,
    step 2, step 3, hello. The lines arrive as they are printed, not at the end.
    Then run on this Mac:
      paddock log -n 1 --json
    Expected: "sandbox": "remote:$linux", "remote_caller" with the Linux sandbox
    id, "remote_host" with the Linux host name, "status": "exit 0".
    A second request on Linux shows no card on either side.
TEXT
  printf 'Did the output stream to Linux, did the Mac card name the Linux host and sandbox, and does the audit line have them? [y/N] '
  read -r ok
  if [ "$ok" = y ]; then echo "PASS seatbelt device behind serve-remote over ssh"; else echo "FAIL seatbelt device behind serve-remote over ssh"; status=1; fi
  if [ -f "$tmp/config.toml.bak" ]; then cp "$tmp/config.toml.bak" "$cfg"; else rm -f "$cfg"; fi
  rm -rf "$tmp"
else
  echo "SKIP step 3 (the ssh round trip from a Linux host)"
fi

if [ "$status" = 0 ]; then
  echo "devices-remote: all checks passed"
else
  echo "devices-remote: a check failed (see FAIL lines above)"
fi
exit "$status"
