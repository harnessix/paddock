#!/bin/sh
# Host devices on a Mac: a device with `run = "host"` runs its program on
# the host as the user, through `paddock device-serve --host`, outside the
# sandbox, with the Mac home, and the marks reach the card, the catalog,
# `paddock device ls`, `paddock doctor`, `paddock ps` and the audit line.
# The code is platform-neutral: the supervisor ends with its parent
# through the parent pid check of macOS, and the socket lives in the
# socket folder of the run (`/tmp/paddock-<uid>/...`). Run it from the
# repository root:
#
#   sh packaging/macos/checks/devices-host.sh
#
# What proves it: every line ends with PASS, and the last line is
# "devices-host: all checks passed". Steps 1 and 2 use temporary folders
# and never touch the user configuration. Step 3 needs a Seatbelt run:
# it adds a device table to the user configuration, starts a run of the
# fake harness, sends one request with `paddock-host device`, and removes
# the table after. It asks before it edits the configuration.
set -u
status=0
out=/tmp/devices-host.out
step() {
  if "$@" >"$out" 2>&1; then
    echo "PASS $*"
  else
    echo "FAIL $*"; sed 's/^/    /' "$out"; status=1
  fi
}

# 1. The marks without a supervisor: the catalog text, the card, the gate
#    that never takes `mode = "allow"`, the source choice, the command
#    line of the supervisor, the state file, the doctor lines and the
#    configuration refusals. Expected: "test result: ok".
step cargo test --lib -- devices::host:: devices::state:: devices::doctor:: devices::config:: cli::device:: cli::device_serve::

# 2. The paddock binary as the supervisor on the host: the program sees
#    the home and the environment, runs in cwd, the request is stdin,
#    timeout and the exit mapping, a serve program once per run that ends
#    with the run (pid gone), the missing cwd and the missing program
#    messages, `ls`, `doctor`, `check`, `ps` and `ps --json`. Expected: 5
#    tests ok. The line "first host request, supervisor start included"
#    is the start time on the Mac.
step cargo test --test devices_host -- --nocapture

# 3. A host exec device in a Seatbelt run: the program runs outside the
#    sandbox with the Mac home, the card shows the host mark as a
#    notification, and `paddock ps` shows the device row. This step is
#    interactive: it adds a device table to the user configuration, opens
#    a Seatbelt shell with the device, and removes the table after. It
#    asks before it edits the configuration.
cfg="$HOME/.config/paddock/config.toml"
printf 'Step 3 adds [devices.mac-home] to %s for the length of the check and removes it after. Continue? [y/N] ' "$cfg"
read -r yes
if [ "$yes" = y ]; then
  tmp=$(mktemp -d)
  cat >"$tmp/mac-home.sh" <<'SH'
#!/bin/sh
echo "home=$HOME"
echo "cwd=$(pwd)"
ls "$HOME/Library" >/dev/null 2>&1 && echo "library=yes"
cat
sleep 5
SH
  chmod 755 "$tmp/mac-home.sh"
  cp "$cfg" "$tmp/config.toml.bak" 2>/dev/null || true
  step cargo run -q --bin paddock -- device add mac-home --run host --exec "$tmp/mac-home.sh"
  step cargo run -q --bin paddock -- device check mac-home
  cat <<TEXT
    A Seatbelt shell opens now with the device. In it, type:
      printf hello | paddock-host device mac-home -
    Expected: a notification "allow the device \`mac-home\` for this sandbox: it runs ON THE HOST as you"
    with the command and the folder in its text. Answer "Allow for this session".
    The response must show status: exit 0, home=$HOME (the Mac home, not a sandbox
    home), cwd=$HOME, library=yes and hello. While the request runs (5 s), run
    in a second terminal:
      cargo run -q --bin paddock -- ps
    Expected: a line "device mac-home (host, running 0m, pid N)" under the sandbox, and
    \`paddock ps --json\` has it under "devices" with "run": "host".
    Then type exit.
TEXT
  cargo run -q --bin paddock -- shell claude --backend seatbelt --device mac-home
  printf 'Did the response show the Mac home and the notification say ON THE HOST, and did ps show the device row? [y/N] '
  read -r ok
  if [ "$ok" = y ]; then echo "PASS seatbelt host device"; else echo "FAIL seatbelt host device"; status=1; fi
  # Remove the table again.
  if [ -f "$tmp/config.toml.bak" ]; then cp "$tmp/config.toml.bak" "$cfg"; else rm -f "$cfg"; fi
  rm -rf "$tmp"
else
  echo "SKIP step 3 (the Seatbelt run)"
fi

if [ "$status" = 0 ]; then
  echo "devices-host: all checks passed"
else
  echo "devices-host: a check failed (see FAIL lines above)"
fi
exit "$status"
