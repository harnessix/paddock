#!/bin/sh
# The device configuration v2 on a Mac: the configuration, the grants and
# `paddock device` are plain code. This script runs the parts that a Mac
# must confirm and prints PASS or FAIL per step. Run it from the
# repository root on the Mac:
#
#   sh packaging/macos/checks/devices-config.sh
#
# What proves it: every line ends with PASS, and the last line is
# "devices-config: all checks passed". Nothing here starts a harness or
# touches the real user configuration: the checks use a temporary
# XDG_CONFIG_HOME.
set -u
status=0
step() {
  if "$@" >/tmp/devices-config.out 2>&1; then
    echo "PASS $*"
  else
    echo "FAIL $*"; sed 's/^/    /' /tmp/devices-config.out; status=1
  fi
}

# 1. The parse, grant, render, doctor and CLI tests run on macOS as on
#    Linux (they are not behind a cfg). Expected: "test result: ok".
step cargo test -- devices::config:: devices::grant:: devices::doctor:: devices::profile:: cli::device:: spawn::ceiling::tests::a_child_gets_at_most_the_devices_of_its_parent manifest::tests pasture::tests

# 2. `paddock device ls` and `show` print the same on a Mac configuration.
#    Expected: ls shows "mac   exec   sandbox  run    on     pasture:workers",
#    show prints the expanded path under the temporary home and no
#    "not available yet" line: a stream runs through the device sandbox.
tmp=$(mktemp -d)
export XDG_CONFIG_HOME="$tmp/cf"
mkdir -p "$tmp/cf/paddock"
cat >"$tmp/cf/paddock/config.toml" <<'TOML'
[devices]
echo = true
[devices.mac]
shape = "exec"
enabled = true
command = ["~/bin/build.sh"]
grant = ["pasture:workers"]
stream = true
[devices.mac.sandbox]
connect = ["2222:mac.local:22"]
ro = ["~/.ssh/known_hosts_mac"]
TOML
check_ls() { cargo run -q --bin paddock -- device ls | grep -q "mac   exec     sandbox  run    on     pasture:workers"; }
check_show() { ! cargo run -q --bin paddock -- device show mac | grep -q "not available yet" && cargo run -q --bin paddock -- device show mac | grep -q "ro          $HOME/.ssh/known_hosts_mac"; }
check_json() { cargo run -q --bin paddock -- device ls --json | grep -q '"host": "mac.local"'; }
step check_ls
step check_show
step check_json

# 3. A second add with the same name is refused; the example script is
#    executable. Expected: the two greps match.
check_add() { cargo run -q --bin paddock -- device add mac --exec x 2>&1 | grep -q "a \[devices.mac\] table exists"; }
check_example() { ( cd "$tmp" && cargo run -q --bin paddock --manifest-path "$OLDPWD/Cargo.toml" -- device example >/dev/null && test -x upper.py ); }
step check_add
step check_example
rm -rf "$tmp"

if [ "$status" = 0 ]; then
  echo "devices-config: all checks passed"
else
  echo "devices-config: a check failed (see FAIL lines above)"
fi
exit "$status"
