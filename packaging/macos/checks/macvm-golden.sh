#!/usr/bin/env bash
# The Mac checks of the golden macOS image. Run this
# on a Mac with Apple silicon after `paddock macvm setup` sealed a golden.
# It prints one line per check, `ok`, `FAIL` or `skip` with the reason,
# and exits non-zero when a check fails.
#
# Usage: packaging/macos/checks/macvm-golden.sh [--no-resume]
#   PADDOCK=path   the paddock binary (default: target/release/paddock,
#                  else paddock on PATH)
#   --no-resume    skip the resume check, which runs a scratch setup for
#                  about a minute (no download: it reuses the restore image)
#
# What proves each check:
#   manifest   state = "sealed" and the version in manifest.toml equal the
#              version of the restore image; the console log of the seal
#              boot names the same version when the guest agent wrote it
#   flags      `ls -lO` shows -r--r--r-- and uchg on disk.img and aux.img
#   seed       the Data volume of the sealed image holds what install.sh of
#              the guided step put there: /Library/Application Support/paddock
#              with launch and current, 501:20 0755, and
#              /Library/LaunchAgents/dev.paddock.guest.plist, 0:0 0644 and
#              valid; no .new or .old leftovers; the seed folder of the
#              golden is gone after the seal
#   space      with PADDOCK_MACVM_FAKE_FREE=1G a scratch setup refuses before
#              the download with the line that names 15 GB and 1 GB
#   resume     a scratch setup killed with -9 during the install leaves state
#              "install" and a .partial; the next setup says "crashed in step 2
#              (install)" and runs the install again; the manifest never said
#              "sealed" in between
#   configure  every verify command of configure.rs prints its expected text
#              in the guest (needs `paddock macvm bench configure-verify`
#              and the guest agent)
#   autologin  a cold boot of a scratch profile reaches `hello` with no login
#              window in under 60 s (needs the session helper)
set -u

status=0
ok()   { printf 'ok    %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1"; status=1; }
skip() { printf 'skip  %s\n' "$1"; }

no_resume=0
for a in "$@"; do
  case "$a" in
    --no-resume) no_resume=1 ;;
    *) echo "unknown argument $a" >&2; exit 2 ;;
  esac
done

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
paddock="${PADDOCK:-$root/target/release/paddock}"
[ -x "$paddock" ] || paddock="$(command -v paddock || true)"
if [ -z "$paddock" ] || [ ! -x "$paddock" ]; then
  echo "paddock binary not found: build it (cargo build --release) or set PADDOCK" >&2
  exit 2
fi
helper="$(dirname "$paddock")/paddock-vm"
if ! /usr/bin/codesign -d --entitlements - --xml "$helper" 2>&1 | grep -q com.apple.security.virtualization; then
  echo "paddock-vm at $helper lacks the virtualization entitlement: codesign -s - --entitlements packaging/macos/paddock-vm.entitlements -f $helper" >&2
  exit 2
fi

status_json="$("$paddock" macvm status --json)" || { echo "paddock macvm status failed" >&2; exit 2; }
# json KEY...: the value at that path of the status JSON, empty when absent.
json() {
  printf '%s' "$status_json" | /usr/bin/python3 -c '
import json, sys
d = json.load(sys.stdin)
for k in sys.argv[1:]:
    d = d.get(k) if isinstance(d, dict) else None
print("" if d is None else d)' "$@" 2>/dev/null
}
dir="$(json dir)"
golden="$dir/golden.noindex"
manifest="$golden/manifest.toml"
state="$(json golden state)"
version="$(json golden version)"

# manifest
if [ "$state" = "sealed" ]; then
  ipsw="$(sed -n 's/^ipsw = "\(.*\)"$/\1/p' "$manifest")"
  if [ -n "$version" ] && grep -q "^state = \"sealed\"" "$manifest"; then
    ok "manifest: state sealed, macOS $version"
  else
    fail "manifest: no version or state in $manifest"
  fi
  if [ -f "$golden/console.log" ] && grep -q "ProductVersion" "$golden/console.log" 2>/dev/null; then
    if grep -q "$version" "$golden/console.log"; then
      ok "manifest: the console log of the seal boot names $version"
    else
      fail "manifest: the console log names another version than $version"
    fi
  else
    skip "manifest: no sw_vers line in the console log yet (the guest agent writes it)"
  fi
  [ -n "$ipsw" ] && case "$ipsw" in *"$version"*) ok "manifest: the restore image $ipsw matches $version" ;; *) skip "manifest: the restore image name $ipsw does not carry the version" ;; esac
else
  fail "manifest: the golden is not sealed (state $state). Run paddock macvm setup first"
fi

# flags
for f in disk.img aux.img; do
  line="$(/bin/ls -lO "$golden/$f" 2>/dev/null)"
  case "$line" in
    -r--r--r--*uchg*) ok "flags: $f is 0444 uchg" ;;
    *) fail "flags: $f: ${line:-missing}" ;;
  esac
done

# seed: attach the sealed image read-only and look at the Data volume.
# The first-boot daemon of the offline seed (or the person, with the typed
# command) installed the agent with install.sh, so the files are in the
# image with the owners and modes of install.sh. hdiutil
# can fail with EAGAIN for a moment after a VM released the file: 5 tries,
# 3 s apart.
attach=""
for try in 1 2 3 4 5; do
  attach="$(/usr/bin/hdiutil attach -imagekey diskimage-class=CRawDiskImage -nobrowse -readonly -owners on "$golden/disk.img" 2>&1)" && break
  case "$attach" in
    *"Resource temporarily unavailable"*) [ "$try" -lt 5 ] && sleep 3 ;;
    *) break ;;
  esac
done
device="$(printf '%s\n' "$attach" | awk -F'\t' '/^\/dev\/disk/ {print $1; exit}' | tr -d ' ')"
device="$(printf '%s' "$device" | sed 's/s[0-9]*$//')"
data=""
while IFS=$'\t' read -r dev hint mp; do
  mp="$(printf '%s' "$mp" | sed 's/^ *//;s/ *$//')"
  [ -n "$mp" ] && [ -d "$mp/Library/LaunchAgents" ] && [ -d "$mp/Library/Application Support/paddock" ] && data="$mp"
done <<< "$attach"
# owner:group mode of a path, as stat prints them.
perm() { /usr/bin/stat -f '%u:%g %Lp' "$1" 2>&1; }
if [ -z "$device" ]; then
  fail "seed: hdiutil attach failed: $attach"
elif [ -z "$data" ]; then
  fail "seed: no Data volume with /Library/Application Support/paddock in the image: neither the first-boot daemon nor the typed command installed the agent"
else
  agent="$data/Library/Application Support/paddock"
  bad=""
  for want in ".:501:20 755" "launch:501:20 755" "current:501:20 755"; do
    rel="${want%%:*}"
    exp="${want#*:}"
    got="$(perm "$agent/$rel")"
    [ "$got" = "$exp" ] || bad="$bad $rel=$got(want $exp)"
  done
  if [ -z "$bad" ]; then
    ok "seed: /Library/Application Support/paddock, launch and current are 501:20 0755"
  else
    fail "seed: agent folder:$bad"
  fi
  if [ -x "$agent/current/paddock-guest" ]; then
    got="$(perm "$agent/current/paddock-guest")"
    [ "$got" = "501:20 755" ] && ok "seed: current/paddock-guest is 501:20 0755" ||
      fail "seed: current/paddock-guest is $got (want 501:20 755)"
  else
    skip "seed: no current/paddock-guest (the guest agent is not in this build)"
  fi
  plist="$data/Library/LaunchAgents/dev.paddock.guest.plist"
  got="$(perm "$plist")"
  if [ "$got" = "0:0 644" ] && /usr/bin/plutil -lint "$plist" >/dev/null 2>&1; then
    ok "seed: dev.paddock.guest.plist root:wheel 0644, valid"
  else
    fail "seed: plist missing, not root:wheel 0644 or not valid: $got"
  fi
  left="$(/bin/ls -d "$agent.new" "$agent.old" "$plist.new" 2>/dev/null)"
  [ -z "$left" ] && ok "seed: no leftovers of install.sh" || fail "seed: leftovers of install.sh: $left"
  [ -e "$golden/seed" ] && fail "seed: the seed folder $golden/seed is still there after the seal" ||
    ok "seed: the seal removed the seed folder"
fi
[ -n "$device" ] && /usr/bin/hdiutil detach "$device" >/dev/null 2>&1

# A scratch data folder for the setups below: a layout of its own, no
# golden, the restore image reused from the real one.
scratch="$(/usr/bin/mktemp -d /tmp/paddock-golden.XXXXXX)"
# PADDOCK_MACVM_IPSW names the image when the setup ran with --ipsw.
ipsw_file="${PADDOCK_MACVM_IPSW:-$(/bin/ls "$dir"/ipsw/*.ipsw 2>/dev/null | head -1)}"

# space
out="$(XDG_DATA_HOME="$scratch/space" PADDOCK_MACVM_FAKE_FREE=1G "$paddock" macvm setup --yes 2>&1)"
code=$?
if [ "$code" -ne 0 ] && printf '%s' "$out" | grep -q "not enough space" && printf '%s' "$out" | grep -q " GB, 1 GB is free"; then
  ok "space: refused before the download: $(printf '%s' "$out" | grep 'not enough space' | head -1)"
else
  fail "space: exit $code, output: $out"
fi
if /bin/ls "$scratch/space/paddock/macvm/ipsw"/* >/dev/null 2>&1; then
  fail "space: a download started although the space check refused"
else
  ok "space: nothing downloaded"
fi

# resume
if [ "$no_resume" = 1 ]; then
  skip "resume: --no-resume"
elif [ -z "$ipsw_file" ]; then
  skip "resume: no restore image in $dir/ipsw to reuse (setup ran with --ipsw: set PADDOCK_MACVM_IPSW)"
else
  sdir="$scratch/resume/paddock/macvm"
  log="$scratch/resume.log"
  XDG_DATA_HOME="$scratch/resume" "$paddock" macvm setup --yes --ipsw "$ipsw_file" >"$log" 2>&1 &
  pid=$!
  sealed_seen=0
  for i in $(seq 1 30); do
    sleep 1
    grep -q '^state = "sealed"' "$sdir/golden.noindex/manifest.toml" 2>/dev/null && sealed_seen=1
  done
  kill -9 "$pid" 2>/dev/null
  sleep 3
  pkill -9 -f "paddock-vm macvm-setup --dir $sdir" 2>/dev/null
  sleep 1
  if grep -q '^state = "install"' "$sdir/golden.noindex/manifest.toml" 2>/dev/null; then
    ok "resume: the manifest says install after kill -9"
  else
    fail "resume: manifest after kill -9: $(cat "$sdir/golden.noindex/manifest.toml" 2>&1 | tr '\n' ' ')"
  fi
  if /bin/ls "$sdir/golden.noindex"/*.partial >/dev/null 2>&1; then
    ok "resume: the install left .partial files"
  else
    fail "resume: no .partial files after kill -9 during the install"
  fi
  XDG_DATA_HOME="$scratch/resume" "$paddock" macvm setup --yes --ipsw "$ipsw_file" >"$log.2" 2>&1 &
  pid=$!
  for i in $(seq 1 20); do
    sleep 1
    grep -q '^state = "sealed"' "$sdir/golden.noindex/manifest.toml" 2>/dev/null && sealed_seen=1
  done
  kill -TERM "$pid" 2>/dev/null
  sleep 2
  pkill -9 -f "paddock-vm macvm-setup --dir $sdir" 2>/dev/null
  if grep -q "crashed in step 2 (install)" "$log.2" && grep -q "step 2 of 6, install" "$log.2"; then
    ok "resume: the next setup removed the partials and ran the install again"
  else
    fail "resume: the next setup did not continue at install: $(head -5 "$log.2" | tr '\n' ' ')"
  fi
  if [ "$sealed_seen" = 0 ]; then
    ok "resume: the manifest never said sealed"
  else
    fail "resume: the manifest said sealed before the seal"
  fi
fi

# configure and autologin need parts that a build may not have yet.
if PADDOCK_MACVM_BENCH=1 "$paddock" macvm bench --help 2>&1 | grep -q configure-verify; then
  if PADDOCK_MACVM_BENCH=1 "$paddock" macvm bench configure-verify; then
    ok "configure: every verify command printed its expected text"
  else
    fail "configure: paddock macvm bench configure-verify failed"
  fi
else
  skip "configure: no paddock macvm bench configure-verify in this build; the setup ran the verify of every configure step itself"
fi
skip "autologin: a cold boot of a scratch profile needs the session helper"

# A clone of the sealed golden keeps its uchg flag: drop it first.
chflags -R nouchg "$scratch" 2>/dev/null
rm -rf "$scratch"
exit "$status"
