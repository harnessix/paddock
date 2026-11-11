#!/bin/sh
# The test of packaging/macos/guest/launch with fake agents, on any Unix
# with /bin/sh. Three cases:
#   good   a binary that exits 0 runs, and the failure count stays 0
#   bad3   a binary that exits 1 at once three times in a row gives
#          prev/: the fourth run is the previous agent, current/ went to
#          broken/, and the launchd log line names the fallback
#   slow   a binary that runs 11 s and then exits 1 does not count as a
#          fast failure: three such runs leave current/ in place
# Each case prints `ok NAME` or `FAIL NAME: reason`. Exit 0 when all pass.
#
# Usage: sh packaging/macos/guest/launch_test.sh
# LAUNCH_TEST_SLOW_SECS (default 11) is how long the slow agent runs.
set -u
here=$(cd "$(dirname "$0")" && pwd)
launch="$here/launch"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/launch-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
fails=0
ok() { echo "ok $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }

# agent DIR NAME BODY: a fake paddock-guest under DIR/NAME/.
agent() {
  mkdir -p "$1/$2"
  printf '#!/bin/sh\necho %s >> "$PADDOCK_GUEST_DIR/ran"\n%s\n' "$2" "$3" > "$1/$2/paddock-guest"
  chmod 755 "$1/$2/paddock-guest"
}
# run DIR: one launchd round. Prints the exit code.
run() {
  PADDOCK_GUEST_DIR="$1" sh "$launch" 2>>"$1/stderr"
  echo $?
}

# good
d="$tmp/good"
agent "$d" current "exit 0"
code=$(run "$d")
if [ "$code" = 0 ] && [ "$(cat "$d/ran")" = current ] && [ "$(cat "$d/.failures")" = 0 ]; then
  ok good
else
  fail good "exit $code, ran=$(cat "$d/ran" 2>/dev/null | tr '\n' ' '), failures=$(cat "$d/.failures" 2>/dev/null)"
fi

# bad3
d="$tmp/bad3"
agent "$d" current "exit 1"
agent "$d" prev "exit 0"
c1=$(run "$d"); c2=$(run "$d"); c3=$(run "$d")
if [ "$c1$c2$c3" != 111 ]; then
  fail bad3 "the three fast failures exited $c1 $c2 $c3"
elif [ -d "$d/prev" ] || [ ! -x "$d/broken/paddock-guest" ]; then
  fail bad3 "after three fast failures prev/ should be current/ and the old current/ broken/: $(ls "$d" | tr '\n' ' ')"
elif [ "$(cat "$d/.failures")" != 0 ]; then
  fail bad3 "the failure count was not reset: $(cat "$d/.failures")"
elif ! grep -q 'prev/ is current/ now' "$d/stderr"; then
  fail bad3 "no fallback line on stderr: $(cat "$d/stderr" | tr '\n' ' ')"
else
  c4=$(run "$d")
  if [ "$c4" = 0 ] && [ "$(tr '\n' ' ' < "$d/ran")" = "current current current prev " ]; then
    ok bad3
  else
    fail bad3 "the fourth run exited $c4 with ran=$(tr '\n' ' ' < "$d/ran")"
  fi
fi

# slow
d="$tmp/slow"
slow="${LAUNCH_TEST_SLOW_SECS:-11}"
agent "$d" current "sleep $slow; exit 1"
agent "$d" prev "exit 0"
c1=$(run "$d"); c2=$(run "$d"); c3=$(run "$d")
if [ "$c1$c2$c3" != 111 ]; then
  fail slow "the three slow failures exited $c1 $c2 $c3"
elif [ ! -d "$d/prev" ] || [ -d "$d/broken" ]; then
  fail slow "a slow failure counted as fast: $(ls "$d" | tr '\n' ' ')"
elif [ "$(cat "$d/.failures")" != 0 ]; then
  fail slow "the failure count is $(cat "$d/.failures"), not 0"
else
  ok slow
fi

# A damaged count file does not break the rule.
d="$tmp/count"
agent "$d" current "exit 1"
echo garbage > "$d/.failures"
run "$d" >/dev/null
if [ "$(cat "$d/.failures")" = 1 ]; then ok count; else fail count "count=$(cat "$d/.failures")"; fi

[ "$fails" = 0 ]
