#!/usr/bin/env bash
# The offline seed and the offline trust of `paddock macvm setup`, checked
# on a Mac. Run it after `packaging/macos/checks/macvm-first-run.sh` (or a
# plain `paddock macvm setup`) on a macOS 27 host, with the sealed golden
# of this worktree. Nothing here needs sudo. Each check prints `ok NAME`,
# `FAIL NAME: why`, `skip NAME: why` or `note ...`, and the script exits 1
# when a check failed.
#
# Usage: packaging/macos/checks/macvm-offline.sh [unit|fixtures|seed|trust|downloads|update|shutdown|all]
#
#   unit      the Mac tests of the offline code: a small APFS image from
#             hdiutil, the attach without owners, the owner patch, fsck_apfs
#             as your user, the clone, and the code requirement against
#             codesign and csreq (cargo test macvm::offline::macos_tests).
#   fixtures  the output of `hdiutil attach -plist` and `diskutil apfs list
#             -plist` on the golden disk has the keys that the reader of
#             src/macvm/diskimage.rs uses; the files go to the output folder
#             to replace src/macvm/testdata/hdiutil-attach.plist and
#             diskutil-apfs-list.plist with recorded ones.
#   seed      the event log says `seed offline` done and names whether
#             fsck_apfs ran; no `agent late`; in the disk the first-boot
#             daemon and its folder are gone, its log says done, the agent
#             folder is 501:20 and the LaunchAgent plist 0:0 0644.
#   trust     the event log says `trust offline` updated, inserted or
#             already, and `trusted`; the agent's Accessibility row in the
#             TCC database of the disk has auth_value 2 and auth_reason 3;
#             the database passes integrity_check and has no -wal or -shm.
#   downloads the event log says `downloads offline` made or already; in the
#             disk Users/paddock/Downloads is a link to
#             "/Volumes/My Shared Files/downloads" with the owner 501:20
#             (the APFS owner patch), and no Downloads folder is left. The
#             setup terminal must show `downloads-link: ok`; a FAIL there
#             with `failed: ... Permission denied` in the event means that
#             the mount without owners still applied the ACL of the folder.
#   update    the requirement of the row is `identifier "dev.paddock.guest"`
#             and the agent binary in the disk has that designated
#             requirement: a self-updated agent signed by build-guest.sh
#             keeps the permission. Then run macvm-upgrade.sh agent-update:
#             its session must still list window titles (they need the
#             permission).
#   shutdown  after one session of PADDOCK_APP (default chatgpt-desktop)
#             that you quit with Cmd-Q: its event log has a quit and no
#             `did not shut down` or `did not stop in` line after it, so
#             the guest stopped itself through `sudo shutdown -h now`
#             within the shutdown wait. Look at the guest window during
#             the quit: no Automation prompt may show.
#
# Environment: PADDOCK (the paddock binary), PADDOCK_DATA (the data folder,
# default ~/Library/Application Support/paddock).
set -u

PADDOCK=${PADDOCK:-paddock}
DATA=${PADDOCK_DATA:-"$HOME/Library/Application Support/paddock"}
MACVM="$DATA/macvm"
OUT=$(mktemp -d "${TMPDIR:-/tmp}/paddock-offline.XXXXXX")
what=${1:-all}
failed=0

ok() { echo "ok $1"; }
fail() {
  echo "FAIL $1: $2"
  failed=1
}
skip() { echo "skip $1: $2"; }
note() { echo "note $*"; }

golden=""
for g in "$MACVM"/golden*; do
  [ -f "$g/manifest.toml" ] && golden="$g" && break
done
events="${golden:+$golden/events.jsonl}"
# event KIND: the texts of the event lines of KIND, one per line.
event() {
  grep "\"kind\":\"$1\"" "$events" 2>/dev/null | sed -E 's/.*"text":"([^"]*)".*/\1/'
}

data=""
device=""
# Attach a clone of the golden disk read-only with owners, so the sealed
# image is never written. The Data volume is the one with Users/.
attach() {
  [ -n "$data" ] && return 0
  local clone="$OUT/disk.img" try a
  cp -c "$golden/disk.img" "$clone" 2>/dev/null || {
    fail attach "cannot clone $golden/disk.img (an APFS volume is needed)"
    return 1
  }
  for try in 1 2 3 4 5; do
    a=$(/usr/bin/hdiutil attach -imagekey diskimage-class=CRawDiskImage -nobrowse -readonly -owners on -mountRoot "$OUT/mnt" -plist "$clone" 2>&1) && break
    sleep 3
  done
  printf '%s\n' "$a" >"$OUT/attach-owners-on.plist"
  device=$(printf '%s\n' "$a" | grep -A1 '<key>dev-entry</key>' | grep -oE '/dev/disk[0-9]+' | head -1)
  for m in "$OUT"/mnt/*; do
    [ -d "$m/Users" ] && [ -d "$m/Library/Application Support" ] && data="$m"
  done
  [ -n "$data" ] || {
    fail attach "no Data volume in $OUT/attach-owners-on.plist"
    return 1
  }
}
detach() {
  [ -n "$device" ] && /usr/bin/hdiutil detach "$device" -force >/dev/null 2>&1
}
trap detach EXIT
perm() { /usr/bin/stat -f '%u:%g %Lp' "$1" 2>&1; }

check_unit() {
  local root
  root=$(cd "$(dirname "$0")/../../.." && pwd)
  if (cd "$root" && cargo test --lib macvm::offline::macos_tests >"$OUT/unit.out" 2>&1); then
    ok "unit ($(grep -E '^test result' "$OUT/unit.out" | head -1))"
  else
    fail unit "see $OUT/unit.out: $(grep -E 'panicked|FAILED' "$OUT/unit.out" | head -3)"
  fi
}

check_fixtures() {
  attach || return
  local a="$OUT/attach-owners-on.plist" l="$OUT/diskutil-apfs-list.plist" k
  /usr/sbin/diskutil apfs list -plist >"$l" 2>&1
  for k in system-entities dev-entry content-hint GUID_partition_scheme Apple_APFS mount-point; do
    grep -q "$k" "$a" && ok "fixtures hdiutil has $k" || fail "fixtures hdiutil has $k" "not in $a"
  done
  grep -qE '41504653-0000-11AA-AA11-00306543ECAC|Apple_APFS_Volume|<string>apfs</string>' "$a" &&
    ok "fixtures hdiutil names APFS volumes" || fail "fixtures hdiutil names APFS volumes" "see $a"
  for k in Containers Volumes Roles DeviceIdentifier '<string>Data</string>'; do
    grep -q "$k" "$l" && ok "fixtures diskutil has $k" || fail "fixtures diskutil has $k" "not in $l"
  done
  note "copy $a to src/macvm/testdata/hdiutil-attach.plist and $l to src/macvm/testdata/diskutil-apfs-list.plist (trim the other disks), then cargo test macvm::diskimage"
}

check_seed() {
  local s late
  s=$(event "seed offline" | tail -n 1)
  case "$s" in
    "done: "*) ok "seed event ($s)" ;;
    *) fail "seed event" "seed offline: ${s:-none}" ;;
  esac
  case "$s" in
    *"fsck clean"*) ok "seed fsck_apfs ran as the user" ;;
    *"fsck skipped"*) note "fsck_apfs did not run as a plain user: ${s#*fsck skipped: }" ;;
  esac
  late=$(event "agent late")
  [ -z "$late" ] && ok "seed agent in time" || fail "seed agent in time" "agent late after $late s"
  attach || return
  [ ! -e "$data/Library/LaunchDaemons/dev.paddock.firstboot.plist" ] && ok "seed daemon removed itself" ||
    fail "seed daemon removed itself" "the plist is still there"
  [ ! -e "$data/Library/Application Support/paddock-firstboot" ] && ok "seed stage removed itself" ||
    fail "seed stage removed itself" "the folder is still there"
  local log="$data/Library/Logs/paddock-firstboot.log"
  if grep -q "agent installed" "$log" 2>/dev/null && grep -q " done" "$log" 2>/dev/null; then
    ok "seed first-boot log"
  else
    fail "seed first-boot log" "$(tail -n 5 "$log" 2>&1)"
  fi
  cp "$log" "$OUT/" 2>/dev/null
  local agent="$data/Library/Application Support/paddock"
  [ "$(perm "$agent")" = "501:20 755" ] && ok "seed agent folder 501:20" || fail "seed agent folder 501:20" "$(perm "$agent")"
  [ "$(perm "$data/Library/LaunchAgents/dev.paddock.guest.plist")" = "0:0 644" ] && ok "seed LaunchAgent 0:0 0644" ||
    fail "seed LaunchAgent 0:0 0644" "$(perm "$data/Library/LaunchAgents/dev.paddock.guest.plist")"
}

tcc_db() { printf '%s' "$data/Library/Application Support/com.apple.TCC/TCC.db"; }
q() { sqlite3 "file:$(tcc_db)?immutable=1" "$1" 2>&1; }

check_trust() {
  local t row
  t=$(event "trust offline" | tail -n 1)
  case "$t" in
    updated | inserted | already) ok "trust event ($t)" ;;
    *) fail "trust event" "trust offline: ${t:-none}" ;;
  esac
  [ -n "$(event trusted)" ] && ok "trust reported" || fail "trust reported" "no trusted line"
  attach || return
  row=$(q "select auth_value, auth_reason from access where service='kTCCServiceAccessibility' and client='/Library/Application Support/paddock/current/paddock-guest' and client_type=1;")
  [ "$row" = "2|3" ] && ok "trust row allowed" || fail "trust row allowed" "row: ${row:-none}"
  [ "$(q 'pragma integrity_check;')" = ok ] && ok "trust integrity" || fail "trust integrity" "$(q 'pragma integrity_check;')"
  local d
  d=$(dirname "$(tcc_db)")
  [ ! -e "$d/TCC.db-wal" ] || [ ! -s "$d/TCC.db-wal" ] && ok "trust no write-ahead log left" ||
    fail "trust no write-ahead log left" "$(ls -l "$d")"
}

check_downloads() {
  local d link
  d=$(event "downloads offline" | tail -n 1)
  case "$d" in
    made | already) ok "downloads event ($d)" ;;
    *) fail "downloads event" "downloads offline: ${d:-none}" ;;
  esac
  attach || return
  link="$data/Users/paddock/Downloads"
  [ -L "$link" ] && ok "downloads is a link" || fail "downloads is a link" "$(ls -ld "$link" 2>&1)"
  [ "$(readlink "$link")" = "/Volumes/My Shared Files/downloads" ] && ok "downloads link target" ||
    fail "downloads link target" "$(readlink "$link" 2>&1)"
  [ "$(/usr/bin/stat -f '%u:%g' "$link" 2>&1)" = "501:20" ] && ok "downloads link 501:20" ||
    fail "downloads link 501:20" "$(/usr/bin/stat -f '%u:%g' "$link" 2>&1)"
}

check_shutdown() {
  local app=${PADDOCK_APP:-chatgpt-desktop} ev tail
  ev="$MACVM/profiles.noindex/$app/default/logs/events.jsonl"
  [ -f "$ev" ] || {
    skip shutdown "no event log at $ev: run one session of $app and quit it"
    return
  }
  tail=$(awk '/"kind":"quit"/ { buf = "" } { buf = buf $0 "\n" } END { printf "%s", buf }' "$ev")
  case "$tail" in
    *'"kind":"quit"'*) ;;
    *)
      skip shutdown "no quit in $ev yet"
      return
      ;;
  esac
  if printf '%s' "$tail" | grep -qE 'did not shut down|did not stop in'; then
    fail shutdown "$(printf '%s' "$tail" | grep -oE '"text":"[^"]*(did not shut down|did not stop in)[^"]*"' | head -1)"
  else
    ok "shutdown by the guest itself"
  fi
}

check_update() {
  attach || return
  q "select hex(csreq) from access where service='kTCCServiceAccessibility' and client='/Library/Application Support/paddock/current/paddock-guest';" |
    xxd -r -p >"$OUT/req.bin"
  local r dr
  r=$(/usr/bin/csreq -r "$OUT/req.bin" -t 2>&1)
  [ "$r" = 'identifier "dev.paddock.guest"' ] && ok "update row requirement" || fail "update row requirement" "$r"
  dr=$(/usr/bin/codesign -d -r- "$data/Library/Application Support/paddock/current/paddock-guest" 2>&1)
  case "$dr" in
    *'designated => identifier "dev.paddock.guest"'*) ok "update agent signature" ;;
    *) fail "update agent signature" "$dr" ;;
  esac
  note "then run packaging/macos/checks/macvm-upgrade.sh agent-update: after the update its windows events must still carry titles"
}

if [ -z "$golden" ]; then
  case "$what" in
    unit) ;;
    *)
      fail pre "no golden under $MACVM: run paddock macvm setup first"
      exit 1
      ;;
  esac
fi
case "$what" in
  unit) check_unit ;;
  fixtures) check_fixtures ;;
  seed) check_seed ;;
  trust) check_trust ;;
  downloads) check_downloads ;;
  update) check_update ;;
  shutdown) check_shutdown ;;
  all)
    check_unit
    check_fixtures
    check_seed
    check_trust
    check_downloads
    check_update
    check_shutdown
    ;;
  *)
    echo "usage: $0 [unit|fixtures|seed|trust|downloads|update|shutdown|all]" >&2
    exit 2
    ;;
esac
echo "output: $OUT"
exit "$failed"
