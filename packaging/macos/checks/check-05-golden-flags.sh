#!/usr/bin/env bash
# Item 5 of the security checklist: the modes. `ls -lO` of the golden
# shows `0444 uchg`, a profile folder is 0700, `state.vzs`, `data.img`
# and every file of a backup are 0600. The Linux test is
# `macvm::security::tests::s05_modes`; a Mac also has the `uchg` flag.
#
# What proves it: the `ls -lO` lines that the script prints, read by the
# `mode` and `flags` helpers, after one session that quit with a save and
# one `paddock app backup`.
set -uo pipefail
item=05
. "$(dirname "$0")/security-lib.sh"
golden_ready
for f in disk.img aux.img; do
  [ "$(mode "$golden/$f")" = 0444 ] || fail "$golden/$f is $(mode "$golden/$f"), not 0444"
  [ "$(flags "$golden/$f")" = uchg ] || fail "$golden/$f has flags $(flags "$golden/$f"), not uchg"
done
note "golden: $(ls -lO "$golden/disk.img" "$golden/aux.img" | awk '{print $1, $5, $NF}' | tr '\n' ';')"
session_up modes || fail "the session did not come up (see $profiles/modes/default/logs/)"
stop modes
p="$profiles/modes/default"
[ -f "$p/state.vzs" ] || fail "no state.vzs after the quit: the save was skipped"
paddock app backup modes >"$tmp/backup.out" 2>&1 || fail "paddock app backup: $(tail -2 "$tmp/backup.out")"
[ "$(mode "$p")" = 0700 ] || fail "the profile folder is $(mode "$p"), not 0700"
for f in state.vzs data.img system.img aux.img machine-id; do
  [ "$(mode "$p/$f")" = 0600 ] || fail "$f is $(mode "$p/$f"), not 0600"
done
n=0
for b in "$p"/backups/*/; do
  [ -d "$b" ] || continue
  [ "$(mode "$b")" = 0700 ] || fail "the backup folder $b is $(mode "$b"), not 0700"
  for f in "$b"*; do
    [ -f "$f" ] || continue
    [ "$(mode "$f")" = 0600 ] || fail "$f is $(mode "$f"), not 0600"
    n=$((n + 1))
  done
done
[ "$n" -ge 5 ] || fail "the backup has $n files, expected the pair, the stamp and backup.json"
note "profile: $(ls -ldO "$p" | awk '{print $1, $5}'); $(ls -lO "$p/state.vzs" "$p/data.img" | awk '{print $1, $5, $NF}' | tr '\n' ';')"
note "backup: $n files 0600 in a 0700 folder"
ok
