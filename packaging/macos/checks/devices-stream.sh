#!/bin/sh
# Device streams on a Mac: the socket framing of a stream and the
# `paddock-host device` client are plain code, and their tests run on
# macOS as on Linux. This script runs them, and names the two checks that
# need a real Seatbelt run with a stream = true device. Run it from the
# repository root on the Mac:
#
#   sh packaging/macos/checks/devices-stream.sh
#
# What proves it: every line ends with PASS, and the last line is
# "devices-stream: all checks passed". Then do the two manual checks at
# the end. Nothing here starts a harness or touches the real user
# configuration.
set -u
status=0
step() {
  if "$@" >/tmp/devices-stream.out 2>&1; then
    echo "PASS $*"
  else
    echo "FAIL $*"; sed 's/^/    /' /tmp/devices-stream.out; status=1
  fi
}

# 1. The socket framing: three chunks and a clean end, a failure mid-way
#    (a close without the 0 chunk) that the client reports as exit 7, an
#    old client (no stream in its header) that gets a collected response
#    with size, and a client that closes a stream and cancels the device
#    within 300 ms. Expected: "test result: ok".
step cargo test -- devices::socket::tests::a_stream_comes_in_chunks_with_a_clean_end devices::socket::tests::a_failure_after_the_first_chunk_is_a_close_without_the_0_chunk devices::socket::tests::an_old_client_gets_a_collected_response_with_a_size devices::socket::tests::a_client_that_closes_a_stream_cancels_the_device

# 2. `paddock-host device` over the socket: each piece printed as it
#    comes, exit 7 after a late failure with the bytes printed, and the
#    exit codes after bytes. Expected: "test result: ok".
step cargo test -- hostcli::device::socket_tests::a_stream_over_the_socket_is_printed_as_it_comes_and_a_late_failure_is_7 hostcli::device::tests::errnos_map_to_the_exit_codes_of_paddock_host

# 3. The stream of `paddock device-serve --stream` through the broker's
#    client: each piece as the program writes it, and a non-zero exit after
#    the first piece as EIO. Expected: "test result: ok".
step cargo test -- devices::outproc::tests::a_stream_device_delivers_each_piece_as_the_program_writes_it devices::supervise::tests

# 4. The catalog on macOS names the stream rule. Expected: the protocol
#    text contains "stream: true answers as it goes".
check_catalog() { cargo test -- devices::tests::the_catalog_on_macos_names_the_command_and_the_fifo devices::tests::catalog_lists_every_device_with_its_state; }
step check_catalog

if [ "$status" = 0 ]; then
  echo "devices-stream: all checks passed"
else
  echo "devices-stream: a check failed (see FAIL lines above)"
fi

cat <<'TEXT'

Manual checks (they need a Seatbelt run with a stream = true device in a Seatbelt device sandbox):

A. In a Seatbelt run with a device table such as
     [devices.build]
     shape = "exec"
     stream = true
     command = ["/bin/sh", "-c", "cargo build 2>&1"]
   run `paddock-host device build` in the sandbox. Expected: the lines of
   `cargo build` appear one by one as cargo prints them, not all at the
   end, and the exit code is 0 (or 7 when the build fails, after the
   lines that came).

B. Start `paddock-host device build` again and close the terminal of the
   agent while the build runs. Expected: `ps` on the host shows no cargo
   of that build within a second, and the audit line of the request says
   "cancelled after N bytes".
TEXT
exit "$status"
