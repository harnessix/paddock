#!/bin/sh
# paddock: the first boot of the macOS guest after the offline seed.
#
# `paddock macvm setup` writes this folder (/Library/Application Support/
# paddock-firstboot) and /Library/LaunchDaemons/dev.paddock.firstboot.plist
# into the guest disk before the first boot, through a mount without owners,
# and then sets the owner of each file to root on the disk itself. launchd
# runs this script once as root at the boot. It:
#
# 1. installs the agent with install.sh --no-start (the agent folder 501:20,
#    the LaunchAgent plist 0:0 0644), the same work as the typed seed command;
# 2. waits for the account paddock (/Users/paddock, made by the macOS 27
#    provisioning or by the person in Setup Assistant) and its login session;
# 3. starts the agent in that session, unless the login loaded it already;
# 4. removes the daemon plist and this folder, so it never runs again.
#
# A failed install leaves the daemon in place: launchd runs the script again
# at the next boot. No step needs a person, a Terminal or a password.
#
# PADDOCK_FIRSTBOOT_ROOT puts every path under a test folder and keeps PATH,
# so the tests can put a fake launchctl and chown first; PADDOCK_FIRSTBOOT_WAIT
# and PADDOCK_FIRSTBOOT_SLEEP replace the hour of the wait and the 2 s between
# the tries. launchd starts the daemon with none of them.
set -u

root=${PADDOCK_FIRSTBOOT_ROOT:-}
if [ -z "$root" ]; then
  PATH=/usr/bin:/bin:/usr/sbin:/sbin
  export PATH
fi
uid=501
user=paddock
label=dev.paddock.guest
here="$root/Library/Application Support/paddock-firstboot"
daemon="$root/Library/LaunchDaemons/dev.paddock.firstboot.plist"
agent_plist="$root/Library/LaunchAgents/$label.plist"
logf="$root/Library/Logs/paddock-firstboot.log"
wait_max=${PADDOCK_FIRSTBOOT_WAIT:-3600}
pause=${PADDOCK_FIRSTBOOT_SLEEP:-2}

mkdir -p "$root/Library/Logs" 2>/dev/null
log() {
  line="paddock firstboot: $(date -u +%Y-%m-%dT%H:%M:%SZ) $*"
  echo "$line"
  echo "$line" >>"$logf" 2>/dev/null
}

log "start"

# 1. The agent files. PADDOCK_SEED_ROOT follows the test root.
if [ -n "$root" ]; then
  PADDOCK_SEED_ROOT=$root PADDOCK_SEED_SLEEP=0 /bin/sh "$here/install.sh" --no-start >>"$logf" 2>&1
else
  /bin/sh "$here/install.sh" --no-start >>"$logf" 2>&1
fi
code=$?
if [ "$code" != 0 ]; then
  log "FAILED: install.sh exit $code; launchd runs this again at the next boot"
  exit 1
fi
log "agent installed"

# 2. The account and its login session.
waited=0
until [ -d "$root/Users/$user" ] && launchctl print "gui/$uid" >/dev/null 2>&1; do
  if [ "$waited" -ge "$wait_max" ]; then
    break
  fi
  sleep "$pause"
  waited=$((waited + pause))
done

# 3. The agent in the session.
if [ -d "$root/Users/$user" ] && launchctl print "gui/$uid" >/dev/null 2>&1; then
  if launchctl print "gui/$uid/$label" >/dev/null 2>&1; then
    log "the login of $user loaded the agent"
  else
    tries=0
    until launchctl bootstrap "gui/$uid" "$agent_plist"; do
      tries=$((tries + 1))
      if [ "$tries" -ge 5 ]; then
        log "launchctl bootstrap gui/$uid failed 5 times; the agent starts at the next login"
        break
      fi
      sleep "$pause"
    done
    [ "$tries" -lt 5 ] && log "agent started in gui/$uid"
  fi
else
  log "no login of $user (uid $uid) within ${wait_max} s; the agent starts at the next login"
fi

# 4. Never again.
rm -f "$daemon"
rm -rf "$here"
log "done"
exit 0
