# paddock escape probe for Seatbelt (macOS). It runs inside the sandbox
# and prints one line per check: "ok <name>", "FAIL <name>: <detail>" or
# "skip <name>: <reason>", as probe.sh does on Linux. Seatbelt denies a
# path but does not hide its name, so each check tries a read or a write,
# not only a lookup. Names shared with probe.sh mean the same check.

ok() { echo "ok $1"; }
fail() { echo "FAIL $1: $2"; }
skip() { echo "skip $1: $2"; }

P="$PADDOCK_PROBE_PROJECT"

# Each argument: can the sandbox list it (a folder) or read it (a file)?
readable() {
  if [ -d "$1" ]; then ls "$1" >/dev/null 2>&1; else head -c 1 "$1" >/dev/null 2>&1; fi
}

# Every name of a colon list that the sandbox can read.
reached() {
  out=""
  old_ifs=$IFS; IFS=:
  for d in $1; do
    [ -n "$d" ] && readable "$d" && out="$out $d"
  done
  IFS=$old_ifs
  echo "$out"
}

if [ -f "$HOME/.paddock-doctor-canary" ]; then ok home-is-sandbox-home; else fail home-is-sandbox-home "HOME is not the sandbox home"; fi

# The real home: no listing, no read, no write.
R="$PADDOCK_PROBE_REALHOME"
if ls "$R" >/dev/null 2>&1; then fail real-home-unreadable "listed $R"; else ok real-home-unreadable; fi
if (echo x > "$R/.paddock-escape-probe") 2>/dev/null; then
  rm -f "$R/.paddock-escape-probe"
  fail real-home-unwritable "wrote into $R"
else
  ok real-home-unwritable
fi
if [ -n "$PADDOCK_PROBE_HOSTFILE" ] && head -c 1 "$PADDOCK_PROBE_HOSTFILE" >/dev/null 2>&1; then
  fail host-home-hidden "read $PADDOCK_PROBE_HOSTFILE"
else
  ok host-home-hidden
fi
# ~/.ssh, ~/.aws and the other secrets of the real home.
leaks=$(reached "$PADDOCK_PROBE_DENIED")
if [ -z "$leaks" ]; then ok protected-paths-absent; else fail protected-paths-absent "readable:$leaks"; fi
if ls "$R/.ssh" >/dev/null 2>&1 || head -c 1 "$R/.ssh/id_ed25519" >/dev/null 2>&1; then
  fail ssh-unreadable "read $R/.ssh"
else
  ok ssh-unreadable
fi
if ls "$R/Documents" >/dev/null 2>&1; then fail documents-unreadable "listed $R/Documents"; else ok documents-unreadable; fi

# Other users: /Users and their homes.
if ls /Users >/dev/null 2>&1; then fail users-unlisted "listed /Users"; else ok users-unlisted; fi
leaks=$(reached "$PADDOCK_PROBE_USERS")
if [ -z "$leaks" ]; then ok other-users-unreadable; else fail other-users-unreadable "readable:$leaks"; fi

# Another sandbox home.
if head -c 1 "$PADDOCK_PROBE_OTHER" >/dev/null 2>&1; then fail other-sandbox-home-hidden "read $PADDOCK_PROBE_OTHER"; else ok other-sandbox-home-hidden; fi

# The host /tmp holds the sockets of other runs. The sandbox has its own TMPDIR.
if head -c 1 "$PADDOCK_PROBE_HOSTTMP" >/dev/null 2>&1; then fail host-tmp-hidden "read a file in the host /tmp"; else ok host-tmp-hidden; fi
if touch "/private/tmp/paddock-escape-probe-$$" 2>/dev/null; then
  rm -f "/private/tmp/paddock-escape-probe-$$"
  fail host-tmp-unwritable "wrote into the host /tmp"
else
  ok host-tmp-unwritable
fi
case "$TMPDIR" in
  /tmp|/tmp/*|/private/tmp|/private/tmp/*|/var/folders/*|/private/var/folders/*|"") fail private-tmpdir "TMPDIR is the host's: '$TMPDIR'" ;;
  *) if (echo x > "$TMPDIR/probe") 2>/dev/null; then rm -f "$TMPDIR/probe"; ok private-tmpdir; else fail private-tmpdir "cannot write TMPDIR $TMPDIR"; fi ;;
esac

# paddock's own state, configuration (approvals) and the sandbox homes.
leaks=$(reached "$PADDOCK_PROBE_PADDOCK")
if [ -z "$leaks" ]; then ok paddock-state-unreadable; else fail paddock-state-unreadable "readable:$leaks"; fi
written=""
old_ifs=$IFS; IFS=:
for d in $PADDOCK_PROBE_PADDOCK; do
  if [ -n "$d" ] && (echo x > "$d/.paddock-escape-probe") 2>/dev/null; then
    rm -f "$d/.paddock-escape-probe"; written="$written $d"
  fi
done
IFS=$old_ifs
if [ -z "$written" ]; then ok paddock-state-unwritable; else fail paddock-state-unwritable "wrote into:$written"; fi

# The project: readable and writable, its secrets masked.
if [ "$(cat "$P/README" 2>/dev/null)" = "paddock doctor project" ]; then ok project-readable; else fail project-readable "cannot read $P/README"; fi
if (echo probe > "$P/.paddock-probe-write") 2>/dev/null && [ "$(cat "$P/.paddock-probe-write")" = probe ]; then
  ok project-writable
else
  fail project-writable "cannot write in $P"
fi
if grep -q paddock-secret-canary "$P/.env" 2>/dev/null; then fail secrets-masked ".env content is readable"; else ok secrets-masked; fi

# The network, by mode.
N="$PADDOCK_PROBE_NETWORK"
nc_ok() { /usr/bin/nc -z -G 3 "$1" "$2" >/dev/null 2>&1; }
if [ "$N" = host ]; then
  if nc_ok 127.0.0.1 "$PADDOCK_PROBE_PORT"; then ok host-loopback-reachable; else fail host-loopback-reachable "network mode host cannot reach a host 127.0.0.1 listener"; fi
else
  if nc_ok 127.0.0.1 "$PADDOCK_PROBE_PORT"; then fail host-loopback-unreachable "connected to a host 127.0.0.1 listener"; else ok host-loopback-unreachable; fi
  if /usr/bin/curl -sS -o /dev/null --max-time 5 --noproxy '*' https://example.com 2>/dev/null; then
    fail direct-egress-blocked "a direct connection bypassed paddock's proxy"
  else
    ok direct-egress-blocked
  fi
fi
case "$N" in
  none)
    if [ -z "$HTTPS_PROXY$HTTP_PROXY$https_proxy$http_proxy" ]; then ok no-proxy-in-mode-none; else fail no-proxy-in-mode-none "a proxy variable is set: $HTTPS_PROXY$HTTP_PROXY"; fi
    ;;
  egress|api|allowlist)
    if [ -z "$HTTPS_PROXY" ]; then
      fail proxy-set "HTTPS_PROXY is empty in network mode $N"
    else
      ok proxy-set
      code=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 5 -x "$HTTPS_PROXY" "http://127.0.0.1:$PADDOCK_PROBE_PORT/" 2>/dev/null)
      case "$code" in
        403) ok proxy-refuses-host ;;
        *) fail proxy-refuses-host "the proxy answered HTTP '$code' for a host loopback port, expected 403" ;;
      esac
    fi
    ;;
esac
if [ -n "$PADDOCK_PROBE_ONLINE" ]; then
  get() { /usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$1" 2>/dev/null; }
  case "$N" in
    none)
      code=$(get https://example.com)
      case "$code" in 000) ok https-blocked ;; *) fail https-blocked "reached example.com (HTTP $code)" ;; esac
      ;;
    egress|host|allowlist)
      code=$(get https://example.com)
      case "$code" in 000) fail https-works "no answer from example.com" ;; *) ok https-works ;; esac
      ;;
  esac
  case "$N" in
    api|allowlist)
      code=$(get https://example.org)
      case "$code" in 000) ok proxy-refuses-unlisted ;; *) fail proxy-refuses-unlisted "reached example.org (HTTP $code)" ;; esac
      ;;
  esac
fi

# Apple's /usr/bin shims (python3, git) start xcrun, which keeps its cache
# in the user temp folder of the host. The sandbox must not write there:
# the host's xcrun trusts that cache. paddock puts the developer tools
# before /usr/bin on PATH, so a command found by PATH skips the shim.
xcrun_check() {
  name=$1; shift
  out=$("$@" 2>&1); rc=$?
  case "$out" in
    *"xcrun: error"*|*"couldn't create cache file"*) fail "$name" "$(echo "$out" | head -n 1)" ;;
    *) if [ "$rc" = 0 ]; then ok "$name"; else fail "$name" "exit code $rc: $(echo "$out" | head -n 1)"; fi ;;
  esac
}
if [ -n "$PADDOCK_PROBE_XCRUN" ]; then
  xcrun_check xcrun-python3 python3 -c pass
  xcrun_check xcrun-git git --version
  # A login shell runs path_helper, which puts /usr/bin first again.
  out=$(/bin/zsh -lc 'python3 -c pass' 2>&1)
  case "$out" in
    *"couldn't create cache file"*) skip xcrun-login-shell "path_helper puts the /usr/bin shims first in a login shell: python3 there prints the xcrun cache error" ;;
    *) ok xcrun-login-shell ;;
  esac
else
  skip xcrun-python3 "no developer tools on this Mac (xcode-select -p)"
fi
