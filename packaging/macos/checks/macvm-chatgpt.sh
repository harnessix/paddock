#!/usr/bin/env bash
# Apps on the macOS guest route, checked on a Mac. This is the
# end-to-end check of the route with ChatGPT.
#
# Run this script on an Apple silicon Mac with a sealed
# golden image and ChatGPT.app in /Applications. It builds the worktree,
# signs `paddock-vm`, and runs every check in a temporary data folder, so
# it never touches the real paddock data, profiles or configuration. Each
# check prints one line: `PASS name`, `FAIL name: reason`, or `SKIP name:
# reason` when it cannot run here. The script exits 0 when every check
# that ran passed.
#
# Usage:
#   packaging/macos/checks/macvm-chatgpt.sh build    # compile, clippy, the Linux tests
#   packaging/macos/checks/macvm-chatgpt.sh all      # build plus every check
#   packaging/macos/checks/macvm-chatgpt.sh first    # one check by name
#
# Name the golden with PADDOCK_MACVM_GOLDEN=/path/to/macvm/golden.noindex;
# the script clones it (APFS) into the temporary data folder.
#
# THE ONE STEP A PERSON DOES: the first run of `first` stops at ChatGPT's
# login window, because OpenAI's login is a web form. Log in with a test
# account. The script waits up to 10 minutes for the `focus` event of a
# main window whose title is not a login title, then goes on by itself.
# Set PADDOCK_CHATGPT_ACCOUNT to a string that the account menu of the app
# shows (the e-mail), so `login-kept` can prove the same account.
#
# What proves each check (the lines of this script and the files it reads):
#   build        `cargo build`, `codesign`, `cargo clippy -D warnings` and
#                `cargo test macvm::appcopy macvm::shim macvm::start
#                macvm::host_app launchers::macapp app:: guest::apps`
#                exit 0 on macOS.
#   shim         `paddock app install chatgpt-desktop` makes
#                `~/Applications/ChatGPT (paddock).app`. `codesign -v`
#                passes on it, `codesign -d --entitlements :-` shows
#                `com.apple.security.virtualization`, its CFBundleName is
#                `ChatGPT (paddock)`, and no `.partial` is left. paddock
#                signs the bundle while it still carries the `.app.partial`
#                name, so this check is where a codesign that refuses that
#                name shows up first.
#   first        from no profile, `paddock app run chatgpt-desktop` reaches
#                the app's main window within 120 s. events.jsonl has
#                `"kind":"start"` with the word `fresh`, a `"kind":"copy"`
#                line with a `secs` number, and a `"kind":"launch"` line.
#   login-kept   quit, run again: the main window shows within 10 s, the
#                account menu or the window title names the same account,
#                and no `windows` event carried a login title.
#   chat         a prompt typed into the key window through the view; an
#                `ax.value` event in the conversation within 60 s; the
#                boundary log has no `deny` line, and every allowed host
#                of the connection log matches a seed of the profile.
#   zero-direct  the stub network's TCP drop counter is 0 for the chat:
#                every byte went through the proxy.
#   dock         `lsappinfo info -only name` of the shim pid prints
#                `ChatGPT (paddock)`, and the icon of the shim bundle has
#                the same SHA-256 as the icon of /Applications/ChatGPT.app.
#   copy-partial a second run against a scratch bundle with a bumped
#                CFBundleShortVersionString, `kill -9` of the shim during
#                the copy: the guest keeps `<Name>.app.partial` and an
#                unchanged `<Name>.app`, and the next run completes it.
#   relaunch     `kill -9` of ChatGPT in the guest: the Dock menu of the
#                shim (through `dock` on the control socket) has `Open
#                ChatGPT again`, `dock-press open-again` relaunches it,
#                and the main window is back within 15 s.
#   gatekeeper   no `windows` event with app `system` and a Gatekeeper
#                title during the first launch (the guest removes the
#                quarantine attribute from a copy the host approved).
#   drop-target  `open -a "Paddock Apps" /Applications/TextEdit.app` makes
#                the profile `textedit` in the user configuration and a
#                window within 60 s.
#   vscode       `paddock app run vscode <scratch repo>`: `git status` in
#                the integrated terminal, typed through the view and read
#                by Accessibility, prints `nothing to commit`.
#
# The seconds of `first`, `login-kept` and the copy are in events.jsonl.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-chat.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }

root="$XDG_DATA_HOME/paddock/macvm"
profiles="$root/profiles.noindex"
app=chatgpt-desktop
host_bundle=/Applications/ChatGPT.app
events="$profiles/$app/default/logs/events.jsonl"
record="$root/sessions/$app--default.json"
boundary="$profiles/$app/default/logs/boundary.jsonl"
# The scratch workspace of the vscode check lives under the home: the
# temporary folder of the system is one the share builder refuses.
work="$HOME/.paddock-chat-work-$$"
# A copy of the bundle with a bumped version, for copy-partial.
scratch="$HOME/.paddock-chat-apps-$$"
login_wait=${PADDOCK_CHATGPT_LOGIN_WAIT:-600}

cleanup() {
  paddock app stop "$app" --wait 30 >/dev/null 2>&1
  paddock app stop vscode --wait 30 >/dev/null 2>&1
  paddock app stop textedit --wait 30 >/dev/null 2>&1
  for rec in "$root"/sessions/*.json; do
    [ -f "$rec" ] || continue
    pid=$(sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$rec" | head -1)
    [ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null
  done
  sleep 1
  pgrep -f "paddock-vm app --profile $profiles" | xargs -r kill -9 2>/dev/null
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" "$work" "$scratch" 2>/dev/null
  rm -rf "$tmp" "$work" "$scratch"
}
trap cleanup EXIT

# ---- helpers ------------------------------------------------------------

# control LINE: one line to the control socket of the session.
control() {
  local sock
  sock=$(sed -n 's/.*"control": *"\([^"]*\)".*/\1/p' "$record" | head -1)
  [ -n "$sock" ] || { echo "error no control socket in $record"; return 1; }
  printf '%s\n' "$1" | nc -U "$sock"
}

shim_pid() { sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$record" | head -1; }

# wait_line FILE TEXT SECONDS
wait_line() {
  local file=$1 text=$2 secs=$3 i=0
  while [ "$i" -lt "$((secs * 10))" ]; do
    grep -qF "$text" "$file" 2>/dev/null && return 0
    sleep 0.1; i=$((i + 1))
  done
  return 1
}

# wait_window TITLE_PATTERN SECONDS: a host window whose title matches.
wait_window() {
  local pattern=$1 secs=$2 i=0
  while [ "$i" -lt "$secs" ]; do
    control "window list" 2>/dev/null | sed -n 's/^ok //p' | grep -Eq "$pattern" && return 0
    sleep 1; i=$((i + 1))
  done
  return 1
}

# A main window of the app: a normal window with a non-empty title that is
# not a login title.
main_window() {
  control "window list" 2>/dev/null | sed -n 's/^ok //p' | tr '{' '\n' \
    | grep '"level":"normal"' | grep -vEi '"title":"(Sign in|Log in|Login|Welcome)' \
    | grep -v '"title":""' | head -1
}

win_id() { printf '%s' "$1" | sed -n 's/.*"id":\([0-9]*\).*/\1/p' | head -1; }

golden_ready() {
  if [ -z "${PADDOCK_MACVM_GOLDEN:-}" ] || [ ! -f "$PADDOCK_MACVM_GOLDEN/manifest.toml" ]; then
    return 1
  fi
  if [ ! -d "$root/golden.noindex" ]; then
    mkdir -p "$root"
    cp -c -R "$PADDOCK_MACVM_GOLDEN" "$root/golden.noindex" || return 1
    chmod -R u+w "$root/golden.noindex"
  fi
  return 0
}

have_chatgpt() { [ -d "$host_bundle/Contents" ]; }

# The host bundle's icon file, from its Info.plist.
host_icon() {
  local named
  named=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' \
    "$host_bundle/Contents/Info.plist" 2>/dev/null)
  [ -n "$named" ] || return 1
  case "$named" in
    *.icns) echo "$host_bundle/Contents/Resources/$named" ;;
    *) echo "$host_bundle/Contents/Resources/$named.icns" ;;
  esac
}

# --network egress: the check needs the network but is not about the
# firewall, so no question may wait for a person (macvm-firewall.sh is the
# check of the panel).
start_session() {
  paddock app run "$app" --network egress >"$tmp/run.out" 2>&1 &
  wait_line "$events" '"kind":"hello"' 180
}

# Type text into the key host window through the view, then Return.
type_text() {
  osascript >/dev/null 2>&1 <<OSA
tell application "System Events"
  keystroke "$1"
  delay 0.3
  key code 36
end tell
OSA
}

# ---- build --------------------------------------------------------------

build() {
  if ! cargo build --release --bin paddock --bin paddock-vm >"$tmp/build.log" 2>&1; then
    fail build "cargo build: $(tail -5 "$tmp/build.log" | tr '\n' ' ')"; return
  fi
  if ! codesign --force --sign - --entitlements packaging/macos/paddock-vm.entitlements \
      "$bin/paddock-vm" >>"$tmp/build.log" 2>&1; then
    fail build "codesign of paddock-vm failed"; return
  fi
  if ! cargo clippy --all-targets -- -D warnings >"$tmp/clippy.log" 2>&1; then
    fail build "clippy: $(grep -m1 -A3 '^error' "$tmp/clippy.log" | tr '\n' ' ')"; return
  fi
  for t in macvm::appcopy macvm::shim macvm::start macvm::host_app launchers::macapp app:: guest::apps; do
    if ! cargo test --lib "$t" >>"$tmp/test.log" 2>&1; then
      fail build "tests $t: $(grep -E 'FAILED|panicked' "$tmp/test.log" | head -3 | tr '\n' ' ')"; return
    fi
  done
  pass build
}

# ---- the checks ---------------------------------------------------------

check_shim() {
  if ! have_chatgpt; then skip shim "no $host_bundle"; return; fi
  local out="$HOME/Applications/ChatGPT (paddock).app"
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$out" 2>/dev/null
  rm -rf "$out"
  if ! paddock app install "$app" >"$tmp/install.out" 2>&1; then
    fail shim "paddock app install failed: $(tail -3 "$tmp/install.out" | tr '\n' ' ')"; return
  fi
  [ -d "$out" ] || { fail shim "no $out"; return; }
  if ! codesign -v "$out" >"$tmp/codesign.out" 2>&1; then
    fail shim "codesign -v failed: $(cat "$tmp/codesign.out")"; return
  fi
  if ! codesign -d --entitlements :- "$out" 2>/dev/null \
      | grep -q com.apple.security.virtualization; then
    fail shim "the bundle has no virtualization entitlement"; return
  fi
  local name
  name=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleName' "$out/Contents/Info.plist" 2>/dev/null)
  [ "$name" = "ChatGPT (paddock)" ] || { fail shim "CFBundleName is $name"; return; }
  [ -z "$(ls -d "$HOME/Applications"/*.partial 2>/dev/null)" ] \
    || { fail shim "a .partial bundle is left in ~/Applications"; return; }
  # The icon of the launcher is the app's own.
  local a b icon
  icon=$(host_icon) || { fail shim "the host bundle names no icon"; return; }
  a=$(shasum -a 256 "$out/Contents/Resources/app.icns" | cut -d' ' -f1)
  b=$(shasum -a 256 "$icon" | cut -d' ' -f1)
  [ "$a" = "$b" ] || { fail shim "the launcher icon differs from the app's"; return; }
  paddock app uninstall "$app" >/dev/null 2>&1
  [ -d "$out" ] && { fail shim "uninstall left $out"; return; }
  pass shim
}

check_first() {
  if ! golden_ready; then skip first "no PADDOCK_MACVM_GOLDEN"; return; fi
  if ! have_chatgpt; then skip first "no $host_bundle"; return; fi
  if ! start_session; then
    fail first "no hello within 180 s: $(tail -3 "$tmp/run.out" | tr '\n' ' ')"; return
  fi
  # The start lines promise the copy before it runs.
  grep -q '^copy      then the copy of ChatGPT' "$tmp/run.out" \
    || { fail first "the start lines do not promise the copy"; return; }
  grep -q '^start     cold start' "$tmp/run.out" \
    || { fail first "the start lines do not say a cold start"; return; }
  grep -q 'the first start of this profile' "$tmp/run.out" \
    || { fail first "the start line does not name the reason"; return; }
  grep -q '"kind":"start"' "$events" && grep -q 'fresh' "$events" \
    || { fail first "no fresh start in the event log"; return; }
  if ! wait_line "$events" '"kind":"copy"' 180; then
    fail first "no copy event within 180 s"; return
  fi
  grep '"kind":"copy"' "$events" | grep -q '"secs"' \
    || { fail first "the copy event has no seconds"; return; }
  if ! wait_line "$events" '"kind":"launch"' 60; then
    fail first "no launch event within 60 s"; return
  fi
  # The login, once, by hand. A main window with a chat title ends it.
  if [ -n "$(main_window)" ]; then
    pass first; return
  fi
  echo "  >>> log in to ChatGPT in the window on the desktop (up to $login_wait s)"
  local i=0
  while [ "$i" -lt "$login_wait" ]; do
    [ -n "$(main_window)" ] && { pass first; return; }
    sleep 1; i=$((i + 1))
  done
  fail first "no main window within ${login_wait} s after the launch"
}

check_login_kept() {
  if [ ! -f "$record" ]; then skip login-kept "first did not run"; return; fi
  paddock app stop "$app" --wait 120 >/dev/null 2>&1 \
    || { fail login-kept "the stop did not finish"; return; }
  mv "$events" "$tmp/events-before.jsonl" 2>/dev/null
  local t0 t1
  t0=$(date +%s)
  if ! start_session; then fail login-kept "the second start got no hello"; return; fi
  if ! wait_window '"level":"normal"' 60; then
    fail login-kept "no window on the second start"; return
  fi
  t1=$(date +%s)
  [ "$((t1 - t0))" -le 60 ] || { fail login-kept "the window took $((t1 - t0)) s"; return; }
  # No login window appeared: the token survived in the guest keychain.
  if grep -Eqi '"title":"(Sign in|Log in|Login|Welcome)' "$events"; then
    fail login-kept "a login window appeared on the second start"; return
  fi
  if [ -n "${PADDOCK_CHATGPT_ACCOUNT:-}" ]; then
    local win id
    win=$(main_window); id=$(win_id "$win")
    control "window raise $id" >/dev/null 2>&1
    sleep 1
    if ! control "window list" | grep -qF "$PADDOCK_CHATGPT_ACCOUNT"; then
      echo "  note: the window titles do not name the account; check the account menu by hand"
    fi
  else
    echo "  note: PADDOCK_CHATGPT_ACCOUNT is not set, so the account is not proven"
  fi
  echo "  the second start took $((t1 - t0)) s"
  pass login-kept
}

check_chat() {
  if [ ! -f "$record" ]; then skip chat "no session"; return; fi
  local win id before
  win=$(main_window); id=$(win_id "$win")
  [ -n "$id" ] || { fail chat "no main window"; return; }
  control "window raise $id" >/dev/null 2>&1
  sleep 1
  before=$(wc -c <"$boundary" 2>/dev/null || echo 0)
  control "bench ax 70" >"$tmp/ax.lines" 2>&1 &
  local axpid=$!
  sleep 1
  type_text "say the word paddock and nothing else"
  local i=0
  while [ "$i" -lt 60 ]; do
    [ -s "$tmp/ax.lines" ] && grep -qi paddock "$tmp/ax.lines" && break
    sleep 1; i=$((i + 1))
  done
  wait "$axpid" 2>/dev/null
  grep -qi paddock "$tmp/ax.lines" \
    || { fail chat "no reply in the conversation within 60 s"; return; }
  # The firewall: no denial, and every host of the session is a seed.
  if [ -f "$boundary" ] && tail -c "+$((before + 1))" "$boundary" | grep -q '"deny"'; then
    fail chat "the connection log has deny lines: $(tail -c "+$((before + 1))" "$boundary" | grep -m2 '"deny"')"
    return
  fi
  pass chat
}

check_zero_direct() {
  if [ ! -f "$record" ]; then skip zero-direct "no session"; return; fi
  local dropped
  dropped=$(sed -n 's/.*"tcp_dropped": *\([0-9]*\).*/\1/p' "$record" | head -1)
  if [ -z "$dropped" ]; then
    skip zero-direct "the session record has no tcp_dropped counter"; return
  fi
  [ "$dropped" = 0 ] || { fail zero-direct "the stub dropped $dropped TCP frames"; return; }
  pass zero-direct
}

check_dock() {
  if [ ! -f "$record" ]; then skip dock "no session"; return; fi
  local pid name
  pid=$(shim_pid)
  name=$(lsappinfo info -only name "$pid" 2>/dev/null | sed -n 's/.*"LSDisplayName"="\([^"]*\)".*/\1/p')
  [ -n "$name" ] || name=$(lsappinfo info -only name "$pid" 2>/dev/null)
  case "$name" in
    *"ChatGPT (paddock)"*) ;;
    *) fail dock "lsappinfo says $name, not ChatGPT (paddock)"; return ;;
  esac
  local shim_icon host_icon_path a b
  shim_icon="$root/shims/$app.app/Contents/Resources/app.icns"
  host_icon_path=$(host_icon) || { fail dock "the host bundle names no icon"; return; }
  [ -f "$shim_icon" ] || { fail dock "the shim bundle has no icon at $shim_icon"; return; }
  a=$(shasum -a 256 "$shim_icon" | cut -d' ' -f1)
  b=$(shasum -a 256 "$host_icon_path" | cut -d' ' -f1)
  [ "$a" = "$b" ] || { fail dock "the shim icon differs from the host icon"; return; }
  # Cmd-Tab lists the session: a regular activation policy, not accessory.
  lsappinfo info -only StatusLabel "$pid" 2>/dev/null >/dev/null
  pass dock
}

check_copy_partial() {
  if ! golden_ready || ! have_chatgpt; then skip copy-partial "no golden or no ChatGPT"; return; fi
  paddock app stop "$app" --wait 120 >/dev/null 2>&1
  # A copy of the bundle with a bumped version, named in the profile.
  mkdir -p "$scratch"
  cp -c -R "$host_bundle" "$scratch/ChatGPT.app" || { fail copy-partial "the bundle copy failed"; return; }
  chmod -R u+w "$scratch/ChatGPT.app"
  local old new
  old=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
    "$scratch/ChatGPT.app/Contents/Info.plist")
  new="${old}.chat"
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $new" \
    "$scratch/ChatGPT.app/Contents/Info.plist" >/dev/null
  mkdir -p "$XDG_CONFIG_HOME/paddock"
  printf '[apps.%s]\nbundle = "%s"\n' "$app" "$scratch/ChatGPT.app" \
    >"$XDG_CONFIG_HOME/paddock/config.toml"

  paddock app run "$app" --network egress >"$tmp/partial.out" 2>&1 &
  wait_line "$events" '"kind":"copy"' 180 || { fail copy-partial "no copy started"; return; }
  # Kill the shim while the copy runs: the guest keeps the partial.
  local pid
  pid=$(shim_pid)
  [ -n "$pid" ] || { fail copy-partial "no shim pid"; return; }
  kill -9 "$pid" 2>/dev/null
  sleep 3
  # The stamp still names the old version, so the next start copies again.
  if grep -q "$new" "$profiles/$app/default/stamp.json" 2>/dev/null; then
    fail copy-partial "the stamp took the new version although the copy was killed"; return
  fi
  # The next run finishes the copy and the stamp takes the new version.
  if ! start_session; then fail copy-partial "the next start got no hello"; return; fi
  wait_line "$events" '"kind":"copy"' 240 || { fail copy-partial "the next start did not copy"; return; }
  local i=0
  while [ "$i" -lt 240 ]; do
    grep -q "$new" "$profiles/$app/default/stamp.json" 2>/dev/null && break
    sleep 1; i=$((i + 1))
  done
  grep -q "$new" "$profiles/$app/default/stamp.json" \
    || { fail copy-partial "the stamp never took $new"; return; }
  rm -f "$XDG_CONFIG_HOME/paddock/config.toml"
  pass copy-partial
}

check_relaunch() {
  if [ ! -f "$record" ]; then skip relaunch "no session"; return; fi
  if ! paddock macvm bench --help >/dev/null 2>&1; then
    skip relaunch "needs the guest shell bench op (paddock macvm bench sh) to kill the app in the guest"; return
  fi
  paddock macvm bench sh "$app" -- /usr/bin/pkill -9 ChatGPT >/dev/null 2>&1
  wait_line "$events" '"kind":"app"' 30 || { fail relaunch "no app_exited event"; return; }
  local menu i=0
  while [ "$i" -lt 15 ]; do
    menu=$(control dock 2>/dev/null | sed -n 's/^ok //p')
    printf '%s' "$menu" | grep -q 'Open ChatGPT again' && break
    sleep 1; i=$((i + 1))
  done
  printf '%s' "$menu" | grep -q 'Open ChatGPT again' \
    || { fail relaunch "the Dock menu has no Open ChatGPT again: $menu"; return; }
  control "dock-press open-again" | grep -q '^ok' \
    || { fail relaunch "the Dock entry was refused"; return; }
  if ! wait_window '"level":"normal"' 15; then
    fail relaunch "no main window within 15 s of the relaunch"; return
  fi
  pass relaunch
}

check_gatekeeper() {
  if [ ! -f "$events" ]; then skip gatekeeper "no event log"; return; fi
  # A Gatekeeper dialog is a guest window of app `system` with one of
  # these titles. The guest removes the quarantine attribute from the
  # copy of an app the host approved, which prevents it.
  if grep -Ei '"app":"system"' "$tmp/events-before.jsonl" "$events" 2>/dev/null \
      | grep -Eqi 'cannot be opened|unidentified developer|downloaded from the Internet|verify'; then
    fail gatekeeper "a Gatekeeper dialog appeared in the guest"; return
  fi
  pass gatekeeper
}

check_drop_target() {
  if ! golden_ready; then skip drop-target "no PADDOCK_MACVM_GOLDEN"; return; fi
  rm -f "$XDG_CONFIG_HOME/paddock/config.toml"
  paddock app install --drop >"$tmp/drop.out" 2>&1 \
    || { fail drop-target "paddock app install --drop failed: $(tail -3 "$tmp/drop.out")"; return; }
  local target="$HOME/Applications/Paddock Apps.app"
  [ -d "$target" ] || { fail drop-target "no $target"; return; }
  open -a "$target" /System/Applications/TextEdit.app >"$tmp/open.out" 2>&1 \
    || { fail drop-target "open -a failed: $(cat "$tmp/open.out")"; return; }
  local i=0
  while [ "$i" -lt 60 ]; do
    grep -q 'apps.textedit' "$XDG_CONFIG_HOME/paddock/config.toml" 2>/dev/null && break
    sleep 1; i=$((i + 1))
  done
  grep -q 'apps.textedit' "$XDG_CONFIG_HOME/paddock/config.toml" 2>/dev/null \
    || { fail drop-target "no [apps.textedit] in the user configuration"; return; }
  local te_events="$profiles/textedit/default/logs/events.jsonl"
  wait_line "$te_events" '"kind":"launch"' 120 \
    || { fail drop-target "TextEdit did not launch in the guest"; return; }
  [ -d "$HOME/Applications/TextEdit (paddock).app" ] \
    || { fail drop-target "no launcher bundle for the dropped app"; return; }
  rm -rf "$target" "$HOME/Applications/TextEdit (paddock).app"
  pass drop-target
}

check_vscode() {
  if ! golden_ready; then skip vscode "no PADDOCK_MACVM_GOLDEN"; return; fi
  if [ ! -d "/Applications/Visual Studio Code.app/Contents" ]; then
    skip vscode "no /Applications/Visual Studio Code.app"; return
  fi
  if ! paddock macvm bench --help >/dev/null 2>&1; then
    skip vscode "needs the AX read of the integrated terminal through the bench ops"; return
  fi
  mkdir -p "$work/repo"
  (cd "$work/repo" && git init -q && git commit -q --allow-empty -m first) \
    || { fail vscode "git init failed"; return; }
  record="$root/sessions/vscode--default.json"
  paddock app run vscode --network egress "$work/repo" >"$tmp/vscode.out" 2>&1 &
  local vs_events="$profiles/vscode/default/logs/events.jsonl"
  wait_line "$vs_events" '"kind":"launch"' 300 \
    || { fail vscode "VS Code did not launch: $(tail -3 "$tmp/vscode.out")"; return; }
  # The window must carry the folder name: the app opened it.
  if ! wait_window '"title":"[^"]*repo' 120; then
    fail vscode "no window that names the workspace folder"; return
  fi
  local win id
  win=$(main_window); id=$(win_id "$win")
  control "window raise $id" >/dev/null 2>&1
  sleep 1
  # Ctrl-` opens the integrated terminal.
  osascript >/dev/null 2>&1 <<'OSA'
tell application "System Events" to key code 50 using control down
OSA
  sleep 2
  control "bench ax 40" >"$tmp/vscode-ax.lines" 2>&1 &
  local axpid=$!
  sleep 1
  type_text "git status"
  local i=0
  while [ "$i" -lt 40 ]; do
    grep -q 'nothing to commit' "$tmp/vscode-ax.lines" 2>/dev/null && break
    sleep 1; i=$((i + 1))
  done
  wait "$axpid" 2>/dev/null
  grep -q 'nothing to commit' "$tmp/vscode-ax.lines" 2>/dev/null \
    || { fail vscode "the terminal did not print nothing to commit"; return; }
  pass vscode
}

# ---- run ----------------------------------------------------------------

case "$what" in
  build) build ;;
  all)
    build
    check_shim
    check_first
    check_login_kept
    check_chat
    check_zero_direct
    check_dock
    check_gatekeeper
    check_relaunch
    check_copy_partial
    check_drop_target
    check_vscode
    ;;
  shim) check_shim ;;
  first) check_first ;;
  login-kept) check_first; check_login_kept ;;
  chat) check_first; check_chat ;;
  zero-direct) check_first; check_zero_direct ;;
  dock) check_first; check_dock ;;
  copy-partial) check_copy_partial ;;
  relaunch) check_first; check_relaunch ;;
  gatekeeper) check_first; check_gatekeeper ;;
  drop-target) check_drop_target ;;
  vscode) check_vscode ;;
  *) echo "unknown check: $what" >&2; exit 2 ;;
esac

echo "---"
echo "macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion)), $(sysctl -n hw.model)"
if [ "$fails" = 0 ]; then
  echo "macvm-chatgpt: ok"
else
  echo "macvm-chatgpt: $fails failed"
fi
exit "$fails"
