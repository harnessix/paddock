#!/usr/bin/env bash
# List the tmux servers that still run with their socket in FOLDER, the
# folder of one test binary. A test stops its tmux servers when it ends
# (TmuxGuard in src/view/guard.rs). A server in this list is a leak.
# test-limit.sh makes FOLDER for each test binary, points TMPDIR and
# PADDOCK_TEST_PARENT at it, runs this script after the binary and fails
# the binary when the list is not empty.
#
# Only a socket below FOLDER counts. A server of another test run, of
# another tool or of the user has its socket outside FOLDER, so the script
# does not list it and does not stop it. The socket file can be gone
# already: the server keeps its listening socket.
#
# Output: one line `PID SOCKET` for each server. With --kill, the script
# also sends TERM to each server that it lists.
#
# Linux only: it reads /proc. On another system it lists nothing.
#
# Usage: tmux-leaks.sh [--kill] FOLDER
set -u

usage() {
  echo "usage: tmux-leaks.sh [--kill] FOLDER" >&2
  exit 2
}

kill=false
if [ "${1:-}" = --kill ]; then
  kill=true
  shift
fi
[ "$#" -eq 1 ] || usage
case "$1" in
  /?*) ;;
  *)
    echo "tmux-leaks: FOLDER must be an absolute path other than /: $1" >&2
    exit 2
    ;;
esac
folder=${1%/}
[ -r /proc/net/unix ] || exit 0

in_folder() {
  case "$1" in
    "$folder"/*) return 0 ;;
  esac
  return 1
}

# The listening unix sockets: inode and path. Flags 00010000 is
# __SO_ACCEPTCON, a socket that listens.
declare -A path_of=()
while read -r _ _ _ flags _ _ inode path; do
  [ "$flags" = 00010000 ] || continue
  [ -n "${path:-}" ] || continue
  path_of[$inode]=$path
done < <(tail -n +2 /proc/net/unix)

for comm in /proc/[0-9]*/comm; do
  read -r name 2>/dev/null <"$comm" || continue
  case "$name" in
    tmux*) ;;
    *) continue ;;
  esac
  dir=${comm%/comm}
  pid=${dir#/proc/}
  for fd in "$dir"/fd/*; do
    link=$(readlink "$fd" 2>/dev/null) || continue
    case "$link" in
      socket:\[*\]) ;;
      *) continue ;;
    esac
    inode=${link#socket:[}
    inode=${inode%]}
    sock=${path_of[$inode]:-}
    [ -n "$sock" ] || continue
    if in_folder "$sock"; then
      echo "$pid $sock"
      if $kill; then
        kill -TERM "$pid" 2>/dev/null
      fi
      break
    fi
  done
done
exit 0
