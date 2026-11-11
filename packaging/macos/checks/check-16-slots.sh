#!/usr/bin/env bash
# Item 16 of the security checklist: the slots folder is sticky and world
# writable, the slot files are 0666, and a holder's text is not trusted
# beyond display. The Linux test is `macvm::security::tests::s16_slots`.
#
# What proves it: `stat` of /private/tmp/dev.paddock.macvm says
# `drwxrwxrwt` and of each slot file `-rw-rw-rw-`; with two sessions up,
# slot-1 is overwritten with 10 KiB of `x` (the lock is flock, the text is
# display only), and a third start exits 2 within 1 s with a refusal line
# that shows the text cut to 80 characters.
set -uo pipefail
item=16
. "$(dirname "$0")/security-lib.sh"
golden_ready
slots=/private/tmp/dev.paddock.macvm
session_up slot-a || fail "slot-a did not come up"
session_up slot-b || fail "slot-b did not come up"
[ "$(stat -f %Sp "$slots")" = drwxrwxrwt ] || fail "$slots is $(stat -f %Sp "$slots"), not drwxrwxrwt"
for i in 0 1; do
  [ "$(stat -f %Sp "$slots/slot-$i")" = -rw-rw-rw- ] || fail "$slots/slot-$i is $(stat -f %Sp "$slots/slot-$i"), not -rw-rw-rw-"
done
note "$(ls -ldO "$slots" | awk '{print $1, $NF}'); $(ls -lO "$slots"/slot-* | awk '{print $1, $NF}' | tr '\n' ';')"
head -c 10240 /dev/zero | tr '\0' x >"$slots/slot-1"
t0=$(date +%s)
paddock app run slot-c --backend macvm --network none >"$tmp/slot-c.out" 2>&1
code=$?
t1=$(date +%s)
[ "$code" = 2 ] || fail "the third start exited $code: $(tail -3 "$tmp/slot-c.out")"
[ $((t1 - t0)) -le 1 ] || fail "the refusal took $((t1 - t0)) s"
line=$(grep -m1 'two macOS VMs already run' "$tmp/slot-c.out")
[ -n "$line" ] || fail "no refusal line: $(tail -3 "$tmp/slot-c.out")"
eighty=$(printf 'x%.0s' $(seq 1 80))
case "$line" in
  *"${eighty}x"*) fail "the holder text is not cut: ${#line} characters" ;;
  *"$eighty"*) ;;
  *) fail "the refusal line does not show the holder text: $line" ;;
esac
[ "${#line}" -lt 400 ] || fail "the refusal line has ${#line} characters"
[ -d "$profiles/slot-c" ] && fail "a folder for slot-c appeared"
note "third start: exit 2 in $((t1 - t0)) s, line of ${#line} characters with the holder cut to 80"
stop slot-a; stop slot-b
ok
