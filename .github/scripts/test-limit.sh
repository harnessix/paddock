#!/usr/bin/env bash
# Run one test binary with a time limit, so a hung test fails its job in
# minutes instead of an hour. In CI, cargo starts every test binary through
# this script: ci.yml names it in CARGO_TARGET_<TRIPLE>_RUNNER.
#
# When the binary runs longer than PADDOCK_TEST_LIMIT seconds (default
# 900), the script stops the binary and the processes that it started,
# names the tests that still ran, and exits with 124. libtest prints
# "test NAME has been running for over 60 seconds" for each slow test. A
# slow test without a result line after that is a test that still ran.
#
# Each binary gets a folder of its own, new and empty: TMPDIR and
# PADDOCK_TEST_PARENT point at it, so the test folders of tempfile and of
# tests/common are in it. After the binary, the script looks for tmux
# servers that still run with a socket in that folder (tmux-leaks.sh). A
# tmux server starts a session of its own, so the stop of the process group
# does not reach it. When there is such a server, the script names it,
# stops it, and fails the binary. The servers of other test runs, of other
# tools and of the user have their sockets outside the folder: the script
# does not count them and does not stop them.
#
# Usage: test-limit.sh BINARY [ARGS...]
set -u

limit=${PADDOCK_TEST_LIMIT:-900}
case "$limit" in
  '' | *[!0-9]*)
    echo "test-limit: PADDOCK_TEST_LIMIT must be a number of seconds: $limit" >&2
    exit 2
    ;;
esac
if [ "$#" -eq 0 ]; then
  echo "usage: test-limit.sh BINARY [ARGS...]" >&2
  exit 2
fi

here=$(cd -- "$(dirname -- "$0")" && pwd)

# The tmux servers that the binary left running. Prints them, stops them,
# and returns 1 when there is one.
tmux_leaks() {
  local new pid sock text
  new=$("$here/tmux-leaks.sh" "$root")
  [ -n "$new" ] || return 0
  text="$(basename "$1") left tmux servers running with a socket in a test folder. A test must stop its tmux server (TmuxGuard in src/view/guard.rs):"
  while read -r pid sock; do
    text="$text $sock (pid $pid);"
    kill -TERM "$pid" 2>/dev/null
  done <<<"$new"
  echo "test-limit: $text" >&2
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::error title=Leaked tmux server::$text"
  fi
  return 1
}

# A short folder: tests/common puts sockets below PADDOCK_TEST_PARENT and
# needs at most 32 bytes for its path. On Linux it is in /var/tmp, where
# tests/common makes its folders without this script.
# The removal at the end stays on the file system of the folder: a test can
# leave a mount in it.
case "$(uname -s)" in
  Linux) base=/var/tmp rm_opts=--one-file-system ;;
  *) base=/tmp rm_opts= ;;
esac
tmp=$(mktemp -d "$base/tl.XXXXXX") || exit 2
trap 'rm -rf $rm_opts -- "$tmp"' EXIT
# The real path, without symlinks (macOS /tmp), as tests/common wants it.
tmp=$(cd -- "$tmp" && pwd -P)
root="$tmp/r"
mkdir -m 700 "$root" || exit 2
out="$tmp/out"
status="$tmp/status"
stopped="$tmp/stopped"

# With job control on, each background job gets a process group of its
# own. A kill of the group reaches the binary, the processes that it
# started and did not move to a new session, and tee.
set -m
{
  TMPDIR="$root" PADDOCK_TEST_PARENT="$root" "$@"
  echo "$?" >"$status"
} 2>&1 | tee "$out" &
tee_pid=$!
group=$(jobs -p %%)

(
  exec >/dev/null 2>&1
  sleep "$limit"
  : >"$stopped"
  kill -TERM -- "-$group"
  sleep 10
  kill -KILL -- "-$group"
) &
dog=$(jobs -p %%)

# Wait for the binary, not for tee. A process that the binary started can
# keep the pipe open after the binary ends, and tee waits for it.
while [ ! -s "$status" ] && [ ! -e "$stopped" ] && kill -0 "$tee_pid" 2>/dev/null; do
  sleep 0.2
done
if [ -e "$stopped" ]; then
  # The watchdog sent TERM. Let it send KILL 10 seconds later.
  wait "$dog" 2>/dev/null
else
  # Give tee a moment to copy the last output, then stop the processes that
  # the binary left behind and the watchdog.
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$tee_pid" 2>/dev/null || break
    sleep 0.2
  done
  # Out of the job table first, so the shell prints no "Killed" notice.
  disown -a
  kill -KILL -- "-$group" 2>/dev/null
  kill -KILL -- "-$dog" 2>/dev/null
fi
wait 2>/dev/null

if [ ! -e "$stopped" ] && [ -s "$status" ]; then
  code=$(cat "$status")
  if ! tmux_leaks "$1" && [ "$code" -eq 0 ]; then
    code=1
  fi
  exit "$code"
fi
if [ ! -e "$stopped" ]; then
  # A signal from outside ended the binary before it wrote its status.
  echo "test-limit: $1 ended without an exit status" >&2
  exit 1
fi

slow=$(sed -n 's/^test \(.*\) has been running for over 60 seconds$/\1/p' "$out")
running=""
while IFS= read -r name; do
  [ -n "$name" ] || continue
  if ! grep -q -F -x -e "test $name ... ok" -e "test $name ... FAILED" -e "test $name ... ignored" "$out"; then
    running="$running $name;"
  fi
done <<EOF
$slow
EOF
binary=$(basename "$1")
if [ -n "$running" ]; then
  text="$binary ran for more than $limit seconds and was stopped. These tests still ran:$running"
else
  text="$binary ran for more than $limit seconds and was stopped. No test ran for more than 60 seconds, so the hang is outside a test or the limit is shorter than 60 seconds."
fi
echo "test-limit: $text" >&2
if [ -n "${GITHUB_ACTIONS:-}" ]; then
  echo "::error title=Test time limit::$text"
fi
exit 124
