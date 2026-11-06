#!/bin/sh
# paddock: install the guest agent into this Mac. `paddock macvm setup`
# writes this file into the seed folder of the golden, and the guided boot
# shares that folder read-only at /Volumes/My Shared Files/paddock-seed.
# The person runs it once in Terminal of the guest, as the account that
# Setup Assistant made (uid 501):
#
#   sudo /bin/sh "/Volumes/My Shared Files/paddock-seed/install.sh"
#
# It copies agent/ of the seed folder to /Library/Application Support/paddock
# (owner 501:20, folders and programs 0755, other files 0644) and the
# launchd plist to /Library/LaunchAgents (owner root:wheel, 0644), then
# starts the agent for uid 501 with launchctl, without a new login. A second
# run does the same work again, so it is safe to run twice. A failure stops
# it with a line that starts with "paddock seed: FAILED".
#
# With --no-start it installs the files and does not start the agent: the
# first-boot daemon of the offline seed (firstboot.sh) runs it that way at
# boot, before anybody logged in, and starts the agent itself after the
# login. launchd also loads the plist at every login by itself.
#
# PADDOCK_SEED_ROOT puts the files under a test folder, skips the root check
# and keeps PATH, so the tests can put a fake chown and launchctl first.
# PADDOCK_SEED_SLEEP replaces the 1 s between the launchctl tries.
# sudo removes both from the environment, so a real run never sees them.
set -eu

uid=501
gid=20
label=dev.paddock.guest
step="start"
start=yes
if [ "${1:-}" = "--no-start" ]; then
  start=no
fi

die() {
  trap - EXIT
  echo "paddock seed: FAILED: $*" >&2
  exit 1
}
trap 'code=$?; [ "$code" = 0 ] || echo "paddock seed: FAILED at \"$step\" (exit $code). Run the same command again: it is safe to run twice." >&2' EXIT

root=${PADDOCK_SEED_ROOT:-}
if [ -z "$root" ]; then
  PATH=/usr/bin:/bin:/usr/sbin:/sbin
  export PATH
  [ "$(id -u)" = 0 ] || die "run it with sudo: sudo /bin/sh \"$0\""
  if [ -n "${SUDO_UID:-}" ] && [ "$SUDO_UID" != "$uid" ]; then
    die "run it as the account that Setup Assistant made (uid $uid), not as uid $SUDO_UID"
  fi
fi

step="read the seed folder"
src=$(cd "$(dirname "$0")" && pwd -P) || die "cannot open the folder of $0"
for f in agent/launch "$label.plist"; do
  [ -f "$src/$f" ] || die "the seed folder $src has no $f. Run paddock macvm setup again on your Mac."
done

dir="$root/Library/Application Support/paddock"
plist="$root/Library/LaunchAgents/$label.plist"

step="copy the agent folder"
mkdir -p "$root/Library/Application Support" "$root/Library/LaunchAgents"
rm -rf "$dir.new" "$dir.old"
cp -R "$src/agent" "$dir.new"
find "$dir.new" -type d -exec chmod 0755 {} +
find "$dir.new" -type f -exec chmod 0644 {} +
chmod 0755 "$dir.new/launch"
if [ -f "$dir.new/current/paddock-guest" ]; then
  chmod 0755 "$dir.new/current/paddock-guest"
else
  echo "paddock seed: no current/paddock-guest in the seed folder; launch waits for an agent" >&2
fi
if [ -z "$root" ]; then
  # A copy can carry extended attributes such as a quarantine flag.
  xattr -cr "$dir.new" 2>/dev/null || true
fi
chown -R "$uid:$gid" "$dir.new"

step="put the agent folder in place"
if [ -e "$dir" ]; then
  mv "$dir" "$dir.old"
fi
mv "$dir.new" "$dir"
rm -rf "$dir.old"

step="install the launchd plist"
rm -f "$plist.new"
cp "$src/$label.plist" "$plist.new"
chmod 0644 "$plist.new"
chown 0:0 "$plist.new"
if [ -z "$root" ]; then
  plutil -lint "$plist.new" >/dev/null || die "the plist in the seed folder is not valid: $(plutil -lint "$plist.new" 2>&1)"
fi
mv -f "$plist.new" "$plist"

if [ "$start" = no ]; then
  trap - EXIT
  echo "paddock seed installed: $dir and $plist. The agent starts at the login of uid $uid."
  exit 0
fi

step="start the agent"
launchctl bootout "gui/$uid/$label" 2>/dev/null || true
launchctl enable "gui/$uid/$label" 2>/dev/null || true
tries=0
until launchctl bootstrap "gui/$uid" "$plist"; do
  tries=$((tries + 1))
  if [ "$tries" -ge 5 ]; then
    die "launchctl bootstrap gui/$uid failed 5 times. The files are in place: log out of the guest and in again, and the agent starts at the login."
  fi
  sleep "${PADDOCK_SEED_SLEEP:-1}"
done

trap - EXIT
echo "paddock seed installed: $dir and $plist. The paddock window marks the step done when the agent dials in."
