#!/bin/sh
# The escape probe of a device sandbox (`paddock doctor --escape --device
# NAME`). paddock runs it as the program of the device, with the profile
# of the named table, and prints its lines: "ok <name>", "FAIL <name>:
# <detail>" or "skip <name>: <reason>". Every "must not" check tries the
# thing a device program must not be able to do. The request is ignored.

ok() { echo "ok $1"; }
fail() { echo "FAIL $1: $2"; }
skip() { echo "skip $1: $2"; }
has() { command -v "$1" >/dev/null 2>&1; }

# tcp HOST PORT: 0 when a connection opens within 3 s, 1 when refused or
# unreachable, 2 when no tool can try.
tcp() {
  if has python3; then
    python3 -c 'import socket,sys
s=socket.socket(); s.settimeout(3)
try:
    s.connect((sys.argv[1], int(sys.argv[2]))); sys.exit(0)
except Exception:
    sys.exit(1)' "$1" "$2"
  elif has bash; then
    timeout 3 bash -c "exec 3<>/dev/tcp/$1/$2" 2>/dev/null
  else
    return 2
  fi
}

cat >/dev/null  # the request

# The home is the empty home of the device sandbox, at the host's path.
n=$(ls -A "$HOME" 2>/dev/null | wc -l | tr -d ' ')
if [ "$n" = 0 ] || [ "$n" = "${PADDOCK_PROBE_HOME_ENTRIES:-0}" ]; then ok home-empty; else fail home-empty "$n entries in $HOME"; fi
if [ -n "${PADDOCK_PROBE_HOSTFILE:-}" ]; then
  if [ -e "$PADDOCK_PROBE_HOSTFILE" ]; then fail host-home-hidden "$PADDOCK_PROBE_HOSTFILE is visible"; else ok host-home-hidden; fi
fi
# The runtime folder of the user holds the sockets of the session and
# paddock's own folder. The sandbox masks it with an empty read-only tmpfs
# at the same path, so the path itself can exist: what counts is that
# nothing of the host shows in it.
if [ -n "${PADDOCK_PROBE_RUNTIME:-}" ]; then
  seen=$(ls -A "$PADDOCK_PROBE_RUNTIME" 2>/dev/null | tr '\n' ' ' | sed 's/ $//')
  if [ -n "$seen" ]; then fail runtime-folder-hidden "$PADDOCK_PROBE_RUNTIME shows: $seen"; else ok runtime-folder-hidden; fi
fi

# The private keys of the agent credentials never enter the sandbox.
old_ifs=$IFS; IFS=:
leaked=""
for k in ${PADDOCK_PROBE_KEYFILES:-}; do
  [ -e "$k" ] && leaked="$leaked $k"
done
IFS=$old_ifs
if [ -n "${PADDOCK_PROBE_KEYFILES:-}" ]; then
  if [ -z "$leaked" ]; then ok key-files-absent; else fail key-files-absent "visible:$leaked"; fi
fi
# ~/.ssh holds the read-only files of the table and nothing else.
if [ -n "${PADDOCK_PROBE_SSH_DIR:-}" ]; then
  got=$(ls -A "$PADDOCK_PROBE_SSH_DIR" 2>/dev/null | sort | tr '\n' ' ' | sed 's/ $//')
  if [ "$got" = "${PADDOCK_PROBE_SSH_EXPECT:-}" ]; then ok ssh-dir-only-ro-files; else fail ssh-dir-only-ro-files "ls $PADDOCK_PROBE_SSH_DIR: '$got', expected '${PADDOCK_PROBE_SSH_EXPECT:-}'"; fi
fi
# The agent socket: there, and it signs only (a delete is refused).
if [ -n "${PADDOCK_PROBE_AGENT:-}" ]; then
  if [ -S "${SSH_AUTH_SOCK:-}" ]; then ok agent-socket-present; else fail agent-socket-present "SSH_AUTH_SOCK=${SSH_AUTH_SOCK:-} is not a socket"; fi
  if has ssh-add; then
    if ssh-add -D >/dev/null 2>&1; then fail agent-filters "ssh-add -D was accepted"; else ok agent-filters; fi
  else
    skip agent-filters "ssh-add is not installed"
  fi
fi

# No paddock-host: a device program is not an agent.
if has paddock-host; then fail no-paddock-host "paddock-host is on the PATH"; else ok no-paddock-host; fi

# The caller's project is read-only.
if [ -n "${PADDOCK_CALLER_MOUNT:-}" ]; then
  if [ -d "$PADDOCK_CALLER_MOUNT" ]; then
    if touch "$PADDOCK_CALLER_MOUNT/.paddock-probe" 2>/dev/null; then
      rm -f "$PADDOCK_CALLER_MOUNT/.paddock-probe"; fail caller-read-only "wrote into $PADDOCK_CALLER_MOUNT"
    else
      ok caller-read-only
    fi
  else
    fail caller-read-only "$PADDOCK_CALLER_MOUNT is not a folder"
  fi
fi

# The network: no interface but lo, no route to the host, the LAN or
# anything else, and only the connect ports on 127.0.0.1 answer.
# /sys may not be in the sandbox: /proc/net/dev lists the same interfaces.
if [ -d /sys/class/net ]; then
  ifs=$(ls /sys/class/net 2>/dev/null | grep -v '^lo$' | tr '\n' ' ')
elif [ -r /proc/net/dev ]; then
  ifs=$(sed -n '3,$s/^ *\([^:]*\):.*/\1/p' /proc/net/dev | grep -v '^lo$' | tr '\n' ' ')
else
  ifs="(cannot list the interfaces: no /sys/class/net, no /proc/net/dev)"
fi
if [ -z "$ifs" ]; then ok no-interface-but-lo; else fail no-interface-but-lo "interfaces: $ifs"; fi
if [ -n "${PADDOCK_PROBE_PORT:-}" ]; then
  tcp 127.0.0.1 "$PADDOCK_PROBE_PORT"; rc=$?
  case $rc in
    0) fail host-loopback-unreachable "connected to a host 127.0.0.1 listener" ;;
    1) ok host-loopback-unreachable ;;
    *) skip host-loopback-unreachable "no python3 and no bash" ;;
  esac
fi
if [ -n "${PADDOCK_PROBE_LAN:-}" ]; then
  tcp "$PADDOCK_PROBE_LAN" 80; rc=$?
  case $rc in
    0) fail lan-unreachable "reached $PADDOCK_PROBE_LAN" ;;
    1) ok lan-unreachable ;;
    *) skip lan-unreachable "no python3 and no bash" ;;
  esac
else
  skip lan-unreachable "no default gateway on the host"
fi
old_ifs=$IFS; IFS=:
for p in ${PADDOCK_PROBE_CONNECT:-}; do
  IFS=$old_ifs
  tcp 127.0.0.1 "$p"; rc=$?
  case $rc in
    0) ok "connect-port-$p-reachable" ;;
    1) fail "connect-port-$p-reachable" "nothing listens on 127.0.0.1:$p inside" ;;
    *) skip "connect-port-$p-reachable" "no python3 and no bash" ;;
  esac
  IFS=:
done
IFS=$old_ifs
echo "probe-done"
