#!/usr/bin/env bash
# Item 11 of the security checklist: no host permission is needed by
# paddock, the shim or the marker. The Linux test is
# `macvm::security::tests::s11_doctor_permissions`.
#
# What proves it: a read-only query of the TCC databases (the user one
# under ~/Library, and the system one when it is readable) finds no row
# whose client is paddock, the shim bundle id prefix `dev.paddock.app` or
# the marker, before and after a session that reached the app launch. The
# user database needs Full Disk Access for the terminal; without it the
# script says so and fails, it does not guess.
set -uo pipefail
item=11
. "$(dirname "$0")/security-lib.sh"
user_db="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
sys_db="/Library/Application Support/com.apple.TCC/TCC.db"
query="select service, client from access where client like '%paddock%' or client like 'dev.paddock.%';"
rows() {
  sqlite3 -readonly "file:$1?mode=ro" "$query" 2>&1
}
command -v sqlite3 >/dev/null || fail "sqlite3 is not installed"
[ -r "$user_db" ] || fail "cannot read $user_db: give the terminal Full Disk Access (System Settings > Privacy & Security), then run again"
before=$(rows "$user_db")
case "$before" in
  *"unable to open"*|*"authorization denied"*) fail "cannot open the user TCC database: $before" ;;
esac
[ -z "$before" ] || fail "TCC rows for paddock before the session: $before"
if [ -r "$sys_db" ]; then
  sys=$(rows "$sys_db")
  [ -z "$sys" ] || fail "system TCC rows for paddock: $sys"
  note "system TCC database: no paddock row"
else
  note "system TCC database not readable without sudo: the user database is what the shim and the marker would get rows in"
fi
golden_ready
session_up tcc || fail "the session did not come up (see $profiles/tcc/default/logs/)"
sleep 5
after=$(rows "$user_db")
[ -z "$after" ] || fail "TCC rows for paddock after the session started: $after"
# The session still runs: a health line younger than 10 s.
paddock app status tcc --json >"$tmp/status.json" 2>&1 || fail "paddock app status: $(tail -2 "$tmp/status.json")"
grep -q '"state": *"running"' "$tmp/status.json" || fail "the session is not running: $(head -c 300 "$tmp/status.json")"
note "no TCC row for paddock, dev.paddock.app* or the marker; the session runs"
stop tcc
ok
