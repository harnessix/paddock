#!/bin/sh
# The Mac check of the guest agent, on a Mac without a guest VM.
#
# What it runs:
#   1. the Linux-portable tests of the agent and the protocol on macOS,
#   2. the release build of paddock-guest with its ad hoc signature and
#      sha256 file (packaging/macos/build-guest.sh),
#   3. the local check `guest-local`: the real agent on this Mac (its
#      AppKit, Accessibility and window server code), a host played by
#      the check over a local socket pair, TextEdit as the app.
#
# What it needs: Apple silicon, Xcode command line tools, the target
# aarch64-apple-darwin, Accessibility granted to the terminal that runs
# it (System Settings > Privacy & Security > Accessibility), and TextEdit
# not running. It writes only under $TMPDIR, enables the German keyboard
# layout for one check and disables it again, and reads (never changes)
# the two accessibility defaults.
#
# What proves what, in the output of step 3:
#   PASS dial      the agent dials, sends hello, takes the token
#   PASS title     a TextEdit window is reported with its title from
#                  kAXTitleAttribute, without Screen Recording
#   PASS move      window.move is answered and the windows event with the
#                  new frame arrives within 50 ms
#   PASS restart   after agent.restart the new agent follows the same
#                  app: the list after the new hello equals the list
#                  before (ids and frames), hello within 2 s
#   PASS hang      with TextEdit stopped (SIGSTOP), window.move replies
#                  Timeout within 2 s and ping still answers within 100 ms
#   PASS guard     configure and data.prepare reply NotInSession while
#                  TextEdit runs
#   PASS input     input.set selects the German layout and a key code 6
#                  (z on US) types y, read back by ax.value from
#                  kAXValueChangedNotification
#   PASS log       log.fetch sends the log file over the files connection
#                  with the token in the header and no token in the lines
#   PASS quit      app.quit ends TextEdit and app_exited follows
#   PASS update    agent.update with a right hash puts the new file in
#                  current/ and the old in prev/, and the agent exits 0
#
# Still open after this script (they need the golden image and the
# session helper `paddock-vm app`): the dial from inside the guest over vsock
# at login, the launchd restart after kill -9, the update to a new hello
# within 5 s through the `agent` share, the rollback by the launch script,
# the log over vsock port 1032, data.prepare on the real second disk, the
# system prompt windows (a TCC dialog) as app `system`, and the pasteboard
# alert of macOS 26.
set -eu
cd "$(dirname "$0")/../../.."
echo "== 1. tests"
cargo test --lib -- guest:: macvm::proto vm::ports
echo "== 2. build"
packaging/macos/build-guest.sh target/guest
test -s target/guest/paddock-guest.sha256
codesign --verify target/guest/paddock-guest && echo "signature ok"
echo "== 3. local check"
cargo run --release --example guest-local
