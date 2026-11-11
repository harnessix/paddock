#!/usr/bin/env bash
# The first run of the macOS guest route, from no golden to a ChatGPT
# login, with every message and only the clicks that a person must make.
#
# Run it on an Apple silicon Mac with /Applications/ChatGPT.app and no
# golden (paddock macvm remove first).
#
# The unattended setup is the default on a macOS 27 host (the guest is
# the newest macOS of the host, so 27 or later too): the person makes no
# click and types nothing until "golden ready". Before the first boot,
# paddock writes the agent and a first-boot daemon into the guest disk
# (the offline seed); the provisioning options make the account; after
# the agent's first report paddock stops the guest, sets the agent's
# Accessibility permission in the guest's TCC database (the offline
# trust) and boots it again. Nothing reads the screen, types or clicks.
# The checks of that mode (see "unattended" below) read the setup event
# log and the terminal:
#   selector  the route is "unattended": the host has
#             VZMacGuestProvisioningOptions and
#             -[VZMacOSVirtualMachineStartOptions setGuestProvisioningOptions:error:],
#             the options passed validateWithError:, and the seed is on
#             the disk.
#   boot      the first boot is "provisioned".
#   seed      "seed offline" is "done": the files are on the disk and owned
#             by root; the agent dialed in with no "agent late" line.
#   trust     "trust offline" is updated, inserted or already, and the
#             agent reported the permission after the second boot;
#             "downloads offline" is made or already.
#   configure every configure step printed "ok".
#   clicks    no line waits on the person ("person":true): the person made
#             no click before golden ready.
# packaging/macos/checks/macvm-offline.sh looks into the disk of the golden
# afterwards: the first-boot log, the owners, the TCC row.
#
# PADDOCK_MACVM_GUIDED=1 forces the guided window and runs the guided
# checks below instead. You then answer Setup
# Assistant with the keyboard English (US) or ABC, enters the password with
# the panel button "Type the password into the guest" (into Password, then
# Verify), runs the install command of panel step 4 in Terminal of the guest
# with "Type the command into the guest" and Return, turns on the
# Accessibility switch and logs in to ChatGPT. The Type buttons are not
# clicks that the setup needs: typing by hand gives the same result, so the
# person waits below stay the same. The script measures the rest and prints
# "ok NAME" or "FAIL NAME: why" per check, and asks a y/n question for what
# only a person sees. It exits 1 when a check fails.
#
# The checks of the guided window:
#   window    the first frame lies inside the visible frame of the main
#             screen (never under the Dock or the menu bar). Run it on a
#             13-inch Mac with the Dock visible at the bottom. Then the
#             script makes the window smaller and asks whether the guest
#             view kept its aspect.
#   typing    a y/n question: the Type buttons filled Password and Verify
#             of Setup Assistant and the Terminal line of step 4.
#   password  only with PADDOCK_CHECK_WRONG_PASSWORD=1: set another
#             password than the panel shows in Setup Assistant (and type it
#             by hand in Terminal). The configure step must say that the
#             guest refused the password, ask for it in the panel, and pass
#             after "Use this password".
#
# Each "want CHECK FUNCTION 'PART'" line asks that the terminal output of
# CHECK holds PART. PART is a fixed part of FUNCTION in src/macvm/text.rs,
# and the Linux test macvm::text first_run_script_matches_texts proves it.
#
# The setup event log: each line that waits on the person carries
# "person":true and a "kind". The kinds must be exactly "account created",
# "seed installed", "trusted" and, on macOS 26 or later, "pasteboard
# allowed". The Linux test macvm::setup_events
# first_run_script_expects_the_person_kinds proves the first three.
#
# Environment: PADDOCK (the paddock binary), PADDOCK_DATA (the paddock data
# folder, default ~/Library/Application Support/paddock).
set -u

PADDOCK=${PADDOCK:-paddock}
DATA=${PADDOCK_DATA:-"$HOME/Library/Application Support/paddock"}
MACVM="$DATA/macvm"
APP=chatgpt-desktop
SHIM="ChatGPT (paddock)"
OUT=$(mktemp -d "${TMPDIR:-/tmp}/paddock-first-run.XXXXXX")
failed=0

ok() { echo "ok $1"; }
fail() {
  echo "FAIL $1: $2"
  failed=1
}

# want CHECK FUNCTION 'PART': the output file of CHECK holds PART.
want() {
  if grep -qF -- "$3" "$OUT/$1.out" 2>/dev/null; then
    ok "$1 $2"
  else
    fail "$1 $2" "no \"$3\" in the output (see $OUT/$1.out)"
  fi
}

# The number of windows of a process, through AX of System Events.
windows_of() {
  osascript -e "tell application \"System Events\" to count windows of (first process whose name is \"$1\")" 2>/dev/null || echo 0
}

# The texts of the setup panel, through AX of the setup process.
panel_texts() {
  osascript -e "tell application \"System Events\" to get value of every static text of entire contents of window 1 of (first process whose unix id is $1)" 2>/dev/null
}

# Ask the person a y/n question on the terminal: "ok NAME" on y.
ask() {
  local answer
  printf '%s [y/n] ' "$2" >/dev/tty
  read -r answer </dev/tty
  case "$answer" in
    y | Y) ok "$1" ;;
    *) fail "$1" "the person answered: ${answer:-nothing}" ;;
  esac
}

# The frame of window 1 of a process as "x y w h", top-left origin.
window_frame() {
  osascript -e "tell application \"System Events\" to tell window 1 of (first process whose unix id is $1) to get (position & size)" 2>/dev/null | tr -d ' ' | tr ',' ' '
}

# The visible frame of the main screen as "x y w h", top-left origin: the
# screen without the menu bar and the Dock.
visible_frame() {
  osascript -l JavaScript -e '
    ObjC.import("AppKit");
    const s = $.NSScreen.mainScreen, f = s.frame, v = s.visibleFrame;
    const top = f.size.height - (v.origin.y + v.size.height);
    [v.origin.x, top, v.size.width, v.size.height].map(Math.round).join(" ");'
}

# window PID: the first frame inside the visible frame, then a resize.
check_window() {
  local x y w h vx vy vw vh
  read -r x y w h <<<"$(window_frame "$1")"
  read -r vx vy vw vh <<<"$(visible_frame)"
  if [ -z "${w:-}" ] || [ -z "${vw:-}" ]; then
    fail "setup window fits the screen" "no frame of the window or the screen"
    return
  fi
  if [ "$x" -ge "$vx" ] && [ "$y" -ge "$vy" ] &&
    [ $((x + w)) -le $((vx + vw)) ] && [ $((y + h)) -le $((vy + vh)) ]; then
    ok "setup window fits the screen ($x $y $w $h in $vx $vy $vw $vh)"
  else
    fail "setup window fits the screen" "window $x $y $w $h, visible $vx $vy $vw $vh"
  fi
  osascript -e "tell application \"System Events\" to tell window 1 of (first process whose unix id is $1) to set size to {$((w - 300)), $((h - 150))}" >/dev/null 2>&1
  ask "setup window resize keeps the aspect" "The window got smaller. Is the guest view still undistorted, with the panel whole at the right, and does the panel scroll to every line?"
}

# Wait until a command prints a non-zero number, at most $1 seconds.
wait_for() {
  local limit=$1 start
  shift
  start=$(date +%s)
  while [ $(($(date +%s) - start)) -lt "$limit" ]; do
    n=$("$@")
    [ "${n:-0}" != 0 ] && return 0
    sleep 0.5
  done
  return 1
}

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  fail pre "run this on an Apple silicon Mac"
  exit 1
fi
[ -d /Applications/ChatGPT.app ] || {
  fail pre "no /Applications/ChatGPT.app"
  exit 1
}
if [ -e "$MACVM/golden/manifest.toml" ]; then
  fail pre "a golden exists: start from none (paddock macvm remove), the script removes nothing"
  exit 1
fi
macos_major=$(sw_vers -productVersion | cut -d. -f1)

# ---- refusal ------------------------------------------------------------
"$PADDOCK" app run "$APP" >"$OUT/refusal.out" 2>&1
code=$?
[ "$code" = 1 ] && ok "refusal exit 1" || fail "refusal exit 1" "exit $code"
want refusal no_golden 'paddock macvm setup'
want refusal no_golden 'GB download'
want refusal no_golden 'minutes ('

# ---- setup --------------------------------------------------------------
mode=guided
if [ "${PADDOCK_MACVM_GUIDED:-0}" != 1 ] && [ "$macos_major" -ge 27 ]; then
  mode=unattended
fi
if [ "$mode" = unattended ]; then
  echo "setup: unattended. Watch the window, do not type or click in it until golden ready (the guest boots twice)"
else
  echo "setup: answer Setup Assistant with the keyboard English (US) or ABC, use Type the password and Type the command of the panel, and turn on the Accessibility switch in the window that opens"
fi
"$PADDOCK" macvm setup --yes >"$OUT/setup.out" 2>&1 &
setup_pid=$!
want_panel_step1=FAIL
saw_banner=no
window_checked=no
order=""
while kill -0 "$setup_pid" 2>/dev/null; do
  texts=$(panel_texts "$setup_pid")
  case "$texts" in *"1. Choose your language and region."*) want_panel_step1=ok ;; esac
  case "$texts" in *"paddock sets up the guest, there is nothing to do in this window"*) saw_banner=yes ;; esac
  if [ "$window_checked" = no ] && [ -n "$texts" ]; then
    window_checked=yes
    check_window "$setup_pid"
  fi
  for n in 2 3 4 5 6; do
    case "$texts" in
      *"✓ $n."*) case " $order " in *" $n "*) ;; *) order="$order $n" ;; esac ;;
    esac
  done
  sleep 1
done
wait "$setup_pid"
code=$?
[ "$code" = 0 ] && ok "setup exit 0" || fail "setup exit 0" "exit $code"
want setup fetch_question 'paddock downloads macOS'
want setup fetch_question 'https://'
want setup fetch_question 'GB download'
want setup fetch_question 'Download and install? [y/N]'
want setup progress '%, about'
want setup golden_ready 'golden ready (macOS '
for phase in download install; do
  pcts=$(grep -oE "^$phase [0-9]+%, about .* left" "$OUT/setup.out" | grep -oE '[0-9]+%' | tr -d % | tr '\n' ' ')
  expect=""
  p=5
  while [ $p -le 100 ]; do
    expect="$expect$p "
    p=$((p + 5))
  done
  [ "$pcts" = "$expect" ] && ok "setup $phase progress every 5 percent" ||
    fail "setup $phase progress every 5 percent" "got: $pcts"
done
[ "$want_panel_step1" = ok ] && ok "setup panel step 1" || fail "setup panel step 1" "the panel never showed step 1"
events=$(ls -t "$MACVM"/golden*/events.jsonl 2>/dev/null | head -n 1)
# event KIND: the texts of the event lines of KIND, one per line.
event() {
  grep "\"kind\":\"$1\"" "$events" 2>/dev/null | sed -E 's/.*"text":"([^"]*)".*/\1/'
}
if [ "$mode" = unattended ]; then
  # ---- unattended -------------------------------------------------------
  want setup seed_offline_done 'nothing to type in the guest'
  want setup trust_reported 'reports the Accessibility permission'
  route=$(event route | tail -n 1)
  case "$route" in
    "unattended "*) ok "unattended selector ($route)" ;;
    *) fail "unattended selector" "route: ${route:-none}. The line of the route says why" ;;
  esac
  boot=$(event "first boot" | head -n 1)
  [ "$boot" = provisioned ] && ok "unattended boot provisioned" || fail "unattended boot provisioned" "first boot: ${boot:-none}"
  seed=$(event "seed offline" | tail -n 1)
  case "$seed" in
    "done: "*) ok "unattended seed ($seed)" ;;
    *) fail "unattended seed" "seed offline: ${seed:-none}" ;;
  esac
  late=$(event "agent late")
  [ -z "$late" ] && ok "unattended agent in time" || fail "unattended agent in time" "agent late after $late s; see /Library/Logs/paddock-firstboot.log of the guest"
  trust=$(event "trust offline" | tail -n 1)
  case "$trust" in
    updated | inserted | already) ok "unattended trust ($trust)" ;;
    *) fail "unattended trust" "trust offline: ${trust:-none}" ;;
  esac
  dl=$(event "downloads offline" | tail -n 1)
  case "$dl" in
    made | already) ok "unattended downloads link ($dl)" ;;
    *) fail "unattended downloads link" "downloads offline: ${dl:-none}" ;;
  esac
  # A quiet step (quiet-*) that did not take is a warning, not a failure:
  # macvm-golden.sh quiet measures those.
  bad=$(grep -E '^configure step [^:]*: FAILED' "$OUT/setup.out" | grep -v '^configure step quiet-')
  steps=$(grep -cE '^configure step [^:]*: ok' "$OUT/setup.out")
  [ -z "$bad" ] && [ "$steps" -gt 0 ] && ok "unattended configure ($steps steps ok)" || fail "unattended configure" "${bad:-no step lines}"
  [ "$saw_banner" = yes ] && ok "unattended banner" || fail "unattended banner" "the panel never showed the unattended line"
  ask "unattended watch" "Did the window show the guest desktop come up twice (a shutdown between), with no dialog left in front and nothing for you to type or click?"
else
ask "setup typing" "Did Type the password fill Password and Verify of Setup Assistant (and the sudo prompt), and Type the command the Terminal line of step 4, each exactly?"
fi
if [ "${PADDOCK_CHECK_WRONG_PASSWORD:-0}" = 1 ] && [ "$mode" = guided ]; then
  want setup configure_wrong_password 'refused the password of the account paddock'
  want setup password_replaced 'keeps the password you typed in its keyring'
  ask "setup password" "Did the panel ask for the password you set, at the top, and did the configure step pass after Use this password?"
fi
[ "$order" = " 2 3 4 5 6" ] && ok "setup steps done in order" || fail "setup steps done in order" "got:$order"

# ---- clicks -------------------------------------------------------------
if [ -z "$events" ]; then
  fail clicks "no events.jsonl of the setup in $MACVM"
elif [ "$mode" = unattended ]; then
  # No wait on the person before golden ready: not one "person":true line.
  waits=$(grep '"person":true' "$events" | sed -E 's/.*"kind":"([^"]*)".*/\1/' | tr '\n' ',')
  [ -z "$waits" ] && ok clicks || fail clicks "the person did: $waits (want nothing)"
else
  waits=$(grep '"person":true' "$events" | sed -E 's/.*"kind":"([^"]*)".*/\1/' | sort | tr '\n' ',')
  expect="account created,seed installed,trusted,"
  [ "$macos_major" -ge 26 ] && expect="account created,pasteboard allowed,seed installed,trusted,"
  [ "$waits" = "$expect" ] && ok clicks || fail clicks "waits on the person: $waits (want $expect)"
fi

# ---- first-run ----------------------------------------------------------
# --network egress: the login must not wait for a person at a firewall
# panel. "first-run seeds" below proves instead that the seeds of the
# profile cover every host of the login, so mode ask would ask nothing.
"$PADDOCK" app run "$APP" --network egress >"$OUT/first-run.out" 2>&1 &
if wait_for 120 windows_of "$SHIM"; then ok "first-run login window"; else fail "first-run login window" "no window of $SHIM in 120 s"; fi
want first-run start_backend 'backend   macvm'
want first-run start_memory 'GiB for the guest'
want first-run start_display 'points at'
want first-run start_shares 'folder'
want first-run start_downloads 'downloads /'
want first-run start_network 'network   egress: every host is allowed'
want first-run start_kind 'cold start, about'
want first-run start_copy 'into the guest, about'
want first-run start_continues 'continues after this terminal closes'
echo "first-run: log in to ChatGPT, then press Enter when the main window shows"
read -r _
"$PADDOCK" app log "$APP" >"$OUT/connections.out" 2>&1
conns="$MACVM/profiles.noindex/$APP/default/logs/connections.jsonl"
# Every host that a process of ChatGPT reached matches a seed of the
# profile: in mode ask none of them would ask.
unseeded=$(/usr/bin/python3 - "$conns" "$(dirname "$0")/../../../profiles/chatgpt-desktop.toml" <<'PY'
import json, re, sys
# /usr/bin/python3 of macOS has no tomllib: read the one array by hand.
text = open(sys.argv[2]).read()
block = re.search(r"seed_hosts\s*=\s*\[(.*?)\]", text, re.S).group(1)
seeds = re.findall(r'"([^"]+)"', re.sub(r"#.*", "", block))
def seeded(h):
    return any(h == s or (s.startswith("*.") and h.endswith(s[1:])) for s in seeds)
out = set()
for line in open(sys.argv[1]):
    try:
        e = json.loads(line)
    except ValueError:
        continue
    if e.get("app", "").startswith("com.openai.chat") and not seeded(e.get("host", "")):
        out.add("%s:%s" % (e.get("host"), e.get("port")))
print(" ".join(sorted(out)))
PY
)
if [ -n "$unseeded" ]; then
  fail "first-run seeds" "the login reached hosts that no seed covers, so mode ask would ask: $unseeded"
else
  ok "first-run seeds"
fi
echo "first-run: hosts of the login (for src/macvm/testdata/chatgpt-first-login.toml):"
grep -oE '"host":"[^"]*","port":[0-9]+' "$OUT/connections.out" | sort -u | sed 's/^/  /'

# ---- warm ---------------------------------------------------------------
"$PADDOCK" app stop "$APP" >/dev/null 2>&1
# Wait until the shim of the session is gone, at most 60 s.
wait_for 60 sh -c "pgrep -f 'shims/$APP.app/' >/dev/null && echo 0 || echo 1" >/dev/null
start=$(date +%s)
"$PADDOCK" app run "$APP" >"$OUT/warm.out" 2>&1 &
if wait_for 10 windows_of "$SHIM"; then
  ok "warm window in $(($(date +%s) - start)) s"
else
  fail "warm window" "no window in 10 s"
fi
want warm start_kind 'warm start, about'
"$PADDOCK" app stop "$APP" >/dev/null 2>&1

echo "output: $OUT"
exit "$failed"
