#!/usr/bin/env bash
# Item 10 of the security checklist: a shim crash leaves no VM, and the
# next start discards the saved state. The Linux test is
# `macvm::security::tests::s10_crash_record`.
#
# What proves it: no `paddock-vm app` process and no
# `com.apple.Virtualization.VirtualMachine` child of it within 2 s of
# `kill -9` of the shim; `paddock app list` says `stopped (crashed)`; the
# next start prints the line of a state that is discarded (the shim log
# has `without a save` or `discarded`); and the data disk is intact. The
# data disk check has two forms. With the guest command door of the agent
# (`paddock macvm control NAME bench launch`), a file written and synced
# in the guest home before the kill reads back after the restart. Without
# the door, the script reads what the host can see: the next start mounts
# the data disk (the stamp keeps `data_prepared` and `app`, so no new
# `data.prepare` and no new app copy happen) and reaches the app launch
# from the copy on the data disk. The `note` line says which form ran.
set -uo pipefail
item=10
. "$(dirname "$0")/security-lib.sh"
golden_ready
p="$profiles/kill9/default"
session_up kill9 || fail "the session did not come up (see $p/logs/)"
door=no
if paddock macvm control kill9 bench launch /usr/bin/true >/dev/null 2>&1; then
  door=yes
  guest kill9 "echo intact-before-kill > $guest_home/.sec10 && sync" >/dev/null
  [ "$(guest kill9 "cat $guest_home/.sec10")" = intact-before-kill ] || fail "the guest did not write the test file through the bench door"
fi
grep -q '"data_prepared": *true' "$p/stamp.json" || fail "the stamp does not say data_prepared after the first launch"
copies_before=$(grep -c '"kind":"copy"' "$(events kill9)")
pid=$(shim_pid kill9)
[ -n "$pid" ] || fail "no shim pid in the session record"
vm=$(pgrep -P "$pid" -f com.apple.Virtualization.VirtualMachine | head -1)
kill -9 "$pid"
sleep 2
pgrep -f "paddock-vm app --profile $p" >/dev/null && fail "a paddock-vm process lives 2 s after kill -9"
[ -n "$vm" ] && kill -0 "$vm" 2>/dev/null && fail "the VirtualMachine process $vm lives 2 s after kill -9"
paddock app list >"$tmp/list.out" 2>&1
grep -q 'kill9.*stopped (crashed)' "$tmp/list.out" || fail "paddock app list after the crash: $(grep kill9 "$tmp/list.out")"
session_up kill9 || fail "the restart did not come up (see $p/logs/)"
grep -Eq 'without a save|discarded' "$p/logs/shim.log" || fail "the restart printed no discard line (shim.log)"
note "restart: $(grep -Em1 'without a save|discarded' "$p/logs/shim.log" | cut -c1-120)"
grep -q '"data_prepared": *true' "$p/stamp.json" || fail "the restart prepared the data disk again: it was not read as the same disk"
copies_after=$(grep -c '"kind":"copy"' "$(events kill9)")
[ "$copies_after" = "$copies_before" ] || fail "the restart copied the app again ($copies_before -> $copies_after copy events): the app copy on the data disk was lost"
if [ "$door" = yes ]; then
  [ "$(guest kill9 "cat $guest_home/.sec10")" = intact-before-kill ] || fail "the test file is not intact after the crash"
  note "data disk: the test file written before the kill reads back intact (bench door)"
else
  note "data disk: no bench door in this build; the restart kept data_prepared and the app copy and reached the launch (the file check needs the guest command door)"
fi
stop kill9
ok
