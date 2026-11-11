#!/usr/bin/env bash
# The menu bar of the app, replicated over Accessibility, checked on a
# Mac.
#
# Run this script on an Apple silicon Mac. It builds the
# worktree, signs `paddock-vm` with its entitlement, starts a session of a
# scratch profile in a temporary data folder (never the real paddock
# data), launches TextEdit in the guest, and reads the main menu of the
# shim process through Accessibility, as the user sees it. Each check
# prints `PASS name`, `FAIL name: reason` or `SKIP name: reason`; the
# script exits 0 when every check that ran passed.
#
# Usage:
#   packaging/macos/checks/macvm-menu.sh build     # compile, clippy, tests
#   packaging/macos/checks/macvm-menu.sh all       # build plus every check
#   packaging/macos/checks/macvm-menu.sh press     # one check by name
#
# What it needs: a sealed golden (`paddock macvm setup`) named by
# PADDOCK_MACVM_GOLDEN, the Xcode command line tools (`swiftc` builds the
# helper that reads and presses the menus of the shim through
# Accessibility and posts keys),
# Accessibility granted to the terminal that runs it, and both VM slots
# free (`switch` runs two sessions). Without the golden, only `build` runs
# and the other checks print SKIP.
#
# The lines of the control socket that the checks use (src/vm/bridge.rs):
#   window list                 the host windows as JSON: id, title, key
#   window raise ID             raise and activate the host window
#   window menu                 the menu state as JSON: shown (`tree` or
#                               `first-pass`), items, away, tree
#   window menu-titles          the titles of the host main menu after the
#                               paddock app menu, from AppKit
#   window menu-press 2.0       press the item with that Accessibility
#                               path, as a click would
#   bench launch PATH           `app.launch` in the guest
#   bench agent-restart         `agent.restart`: the agent exits and
#                               launchd starts it again (a kill -9 of the
#                               agent needs a shell in the guest, which
#                               the host has not; the control connection
#                               closes the same way)
#
# What proves each check:
#   build    `cargo build`, `cargo clippy -D warnings` and `cargo test
#            macvm::menu macvm::proto macvm::text guest::` exit 0 on macOS.
#   tree     from the launch of TextEdit, polled every 50 ms, the main
#            menu of the shim process (read through AX, `menubar PID`) has
#            `File`, `Edit`, `Format`, `View` and `Window` at most 500 ms
#            after the first host window appeared in `window list`; the
#            paddock app menu is first; `window menu` says `tree`; the
#            File menu has `New` with the key equivalent `n` and Command.
#   press    `Format > Make Plain Text` pressed through the host menu (AX
#            `AXPress` on the item of the shim process) changes the
#            document: within 500 ms the Format menu of the host shows
#            `Make Rich Text` instead, which only the guest's TextEdit can
#            cause, and events.jsonl has `menu press Format > Make Plain
#            Text`. Then a stale path (`window menu-press 2.99`) gives
#            `menu press dropped` in events.jsonl and no `menu press
#            TITLES` line after it.
#   switch   a second session with Calculator: a raise of its window puts
#            its menus (`View` without `Format`) in front within 100 ms,
#            polled every 10 ms through AX of the frontmost application,
#            and a raise of the TextEdit window brings `Format` back
#            within 100 ms.
#   away     after `bench agent-restart`, the host menu keeps `Format`
#            on every sample until the new `hello` (events.jsonl), `window
#            menu` says `tree` on every sample, and a new `menu of` line
#            follows the hello within 2 s.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-menu.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profile="$root/profiles.noindex/menu/default"
events="$profile/logs/events.jsonl"
record="$root/sessions/menu--default.json"
profile_b="$root/profiles.noindex/menub/default"
record_b="$root/sessions/menub--default.json"

cleanup() {
  paddock app stop menu >/dev/null 2>&1
  paddock app stop menub >/dev/null 2>&1
  sleep 2
  pgrep -f "paddock-vm app --profile $profile" | xargs -r kill -9 2>/dev/null
  pgrep -f "paddock-vm app --profile $profile_b" | xargs -r kill -9 2>/dev/null
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

# The Swift helper: the menus of a process through Accessibility, the
# frontmost application, and synthetic keys.
cat >"$tmp/menu.swift" <<'EOF'
import Foundation
import AppKit
import ApplicationServices
let a = CommandLine.arguments
func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
  var v: AnyObject?
  return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
}
func children(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] }
func title(_ e: AXUIElement) -> String { (attr(e, kAXTitleAttribute) as? String) ?? "" }
func app(_ pid: String) -> AXUIElement {
  let e = AXUIElementCreateApplication(pid_t(Int32(pid)!))
  AXUIElementSetMessagingTimeout(e, 1.0)
  return e
}
func bar(_ e: AXUIElement) -> [AXUIElement] {
  guard let b = attr(e, kAXMenuBarAttribute) else { return [] }
  return children(b as! AXUIElement)
}
func menu(_ e: AXUIElement, _ name: String) -> [AXUIElement] {
  guard let top = bar(e).first(where: { title($0) == name }), let m = children(top).first else { return [] }
  return children(m)
}
switch a[1] {
case "menubar":
  // menubar PID: the titles of the menu bar items of the process, one per line.
  for t in bar(app(a[2])) { print(title(t)) }
case "menuitems":
  // menuitems PID MENU: the titles of the items of that menu, one per line.
  for i in menu(app(a[2]), a[3]) { print(title(i)) }
case "menukey":
  // menukey PID MENU ITEM: the command character and the modifier mask of the item.
  guard let i = menu(app(a[2]), a[3]).first(where: { title($0) == a[4] }) else { print("no item"); exit(1) }
  print("\((attr(i, kAXMenuItemCmdCharAttribute) as? String) ?? "") \((attr(i, kAXMenuItemCmdModifiersAttribute) as? Int) ?? -1)")
case "press":
  // press PID MENU ITEM: AXPress on the item of that menu.
  guard let i = menu(app(a[2]), a[3]).first(where: { title($0) == a[4] }) else { print("no item \(a[4]) in \(a[3])"); exit(1) }
  let r = AXUIElementPerformAction(i, kAXPressAction as CFString)
  if r != .success { print("press failed: \(r.rawValue)"); exit(1) }
  print("pressed")
case "front":
  // front: the pid of the frontmost application and its menu bar titles on one line.
  guard let f = NSWorkspace.shared.frontmostApplication else { print("0"); exit(0) }
  let titles = bar(app(String(f.processIdentifier))).map(title).joined(separator: ",")
  print("\(f.processIdentifier) \(titles)")
case "key":
  // key CODE [cmd]: a key down and up, with Command when asked.
  let code = CGKeyCode(UInt16(a[2])!)
  let cmd = a.count > 3 && a[3] == "cmd"
  let d = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)
  let u = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)
  if cmd { d?.flags = .maskCommand; u?.flags = .maskCommand }
  d?.post(tap: .cghidEventTap); usleep(30_000); u?.post(tap: .cghidEventTap)
default:
  print("unknown"); exit(2)
}
EOF
# Compiled once: a `swift FILE` run starts in half a second, too slow for
# the 10 ms polls of `switch`.
helper() {
  if [ ! -x "$tmp/menu" ]; then
    swiftc -O -o "$tmp/menu" "$tmp/menu.swift" >"$tmp/swiftc.log" 2>&1 || { echo "swiftc failed: $(tail -3 "$tmp/swiftc.log")" >&2; return 1; }
  fi
  "$tmp/menu" "$@"
}

build() {
  if ! cargo build --release --bin paddock --bin paddock-vm >"$tmp/build.log" 2>&1; then
    fail build "cargo build: $(tail -5 "$tmp/build.log" | tr '\n' ' ')"; return
  fi
  if ! codesign --force --sign - --entitlements packaging/macos/paddock-vm.entitlements "$bin/paddock-vm" >>"$tmp/build.log" 2>&1; then
    fail build "codesign of paddock-vm failed"; return
  fi
  if ! cargo clippy --all-targets -- -D warnings >"$tmp/clippy.log" 2>&1; then
    fail build "clippy: $(grep -m1 -A3 '^error' "$tmp/clippy.log" | tr '\n' ' ')"; return
  fi
  for t in macvm::menu macvm::proto macvm::text guest::; do
    if ! cargo test --lib -- "$t" >>"$tmp/test.log" 2>&1; then
      fail build "cargo test $t: $(grep -E 'FAILED|panicked' "$tmp/test.log" | head -3 | tr '\n' ' ')"; return
    fi
  done
  pass build
}

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

# control RECORD LINE: one line to the control socket of that session,
# the answer on stdout.
control() {
  local sock
  sock=$(sed -n 's/.*"control": *"\([^"]*\)".*/\1/p' "$1" | head -1)
  [ -n "$sock" ] || { echo "error no control socket in $1"; return 1; }
  printf '%s\n' "$2" | nc -U "$sock"
}
wins() { control "$1" "window list" | sed -n 's/^ok //p'; }
ids() { printf '%s' "$1" | tr '{' '\n' | sed -n 's/.*"id":\([0-9]*\)[,}].*/\1/p'; }
shim_pid() { sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$1" | head -1; }
now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }
count_lines() { grep -c "$2" "$1" 2>/dev/null || echo 0; }
# wait_count FILE PATTERN N SECONDS: until the file has more than N lines with the pattern.
wait_count() {
  local i=0 n; n=$(awk "BEGIN{print int($4 * 10)}")
  while [ "$i" -lt "$n" ]; do
    [ "$(count_lines "$1" "$2")" -gt "$3" ] && return 0
    sleep 0.1; i=$((i + 1))
  done
  return 1
}
# menu_shown RECORD: the `shown` word of `window menu`.
menu_shown() { control "$1" "window menu" | sed -n 's/^ok .*"shown":"\([^"]*\)".*/\1/p'; }
has_titles() { for t in File Edit Format View Window; do printf '%s\n' "$1" | grep -qx "$t" || return 1; done; return 0; }

# session_up NAME RECORD APP_PATH: a session with that app and one window.
# Sets $t_window (ms when the first host window appeared) and $key.
session_up() {
  local name=$1 rec=$2 app=$3
  if [ -f "$rec" ] && kill -0 "$(shim_pid "$rec")" 2>/dev/null; then
    key=$(ids "$(wins "$rec")" | head -1); return 0
  fi
  rm -f "$rec"
  paddock app run "$name" --backend macvm --network none >"$tmp/$name.out" 2>&1 &
  local i=0
  while [ "$i" -lt 900 ]; do
    [ -f "$rec" ] && grep -q '"kind":"hello"' "$(dirname "$(dirname "$rec")")/profiles.noindex/$name/default/logs/events.jsonl" 2>/dev/null && break
    sleep 0.1; i=$((i + 1))
  done
  [ "$i" -lt 900 ] || { echo "no hello within 90 s (see the logs under $root)"; return 1; }
  control "$rec" "bench launch $app" | grep -q '^ok' || { echo "bench launch refused"; return 1; }
  t_window=""
  i=0
  while [ "$i" -lt 300 ]; do
    if [ -n "$(ids "$(wins "$rec")")" ]; then t_window=$(now_ms); break; fi
    sleep 0.05; i=$((i + 1))
  done
  [ -n "$t_window" ] || { echo "$app showed no window in 15 s"; return 1; }
  key=$(ids "$(wins "$rec")" | head -1)
  return 0
}

check_tree() {
  session_up menu "$record" /System/Applications/TextEdit.app >"$tmp/up.log" 2>&1 || { fail tree "no session: $(cat "$tmp/up.log")"; return; }
  local pid; pid=$(shim_pid "$record")
  local i=0 titles="" t_menu=""
  while [ "$i" -lt 100 ]; do
    titles=$(helper menubar "$pid")
    if has_titles "$titles"; then t_menu=$(now_ms); break; fi
    sleep 0.05; i=$((i + 1))
  done
  [ -n "$t_menu" ] || { fail tree "the host menu bar never had File Edit Format View Window: $(printf '%s' "$titles" | tr '\n' ',')"; return; }
  local ms=$((t_menu - t_window))
  [ "$ms" -le 500 ] || { fail tree "the menus came $ms ms after the first window (limit 500)"; return; }
  # The paddock app menu is first: not one of the app's menus.
  local first; first=$(printf '%s\n' "$titles" | sed -n 2p)
  [ "$first" != "File" ] || { fail tree "the first menu is File, not the paddock app menu"; return; }
  [ "$(menu_shown "$record")" = tree ] || { fail tree "window menu says $(menu_shown "$record"), not tree"; return; }
  local new; new=$(helper menukey "$pid" File New)
  [ "$new" = "n 0" ] || { fail tree "File > New has the key '$new', not 'n 0' (Command-N)"; return; }
  grep -q '"menu of' "$events" || { fail tree "no 'menu of' line in events.jsonl"; return; }
  pass tree
}

check_press() {
  session_up menu "$record" /System/Applications/TextEdit.app >"$tmp/up.log" 2>&1 || { fail press "no session: $(cat "$tmp/up.log")"; return; }
  local pid; pid=$(shim_pid "$record")
  control "$record" "window raise $key" >/dev/null
  sleep 0.5
  local items; items=$(helper menuitems "$pid" Format)
  local target=""; local other=""
  if printf '%s\n' "$items" | grep -qx "Make Plain Text"; then target="Make Plain Text"; other="Make Rich Text"; fi
  if printf '%s\n' "$items" | grep -qx "Make Rich Text"; then target="Make Rich Text"; other="Make Plain Text"; fi
  [ -n "$target" ] || { fail press "the Format menu has neither Make Plain Text nor Make Rich Text: $(printf '%s' "$items" | tr '\n' ',')"; return; }
  local presses; presses=$(count_lines "$events" '"menu press Format')
  local out; out=$(helper press "$pid" Format "$target" 2>&1) || { fail press "AXPress failed: $out"; return; }
  local t0; t0=$(now_ms)
  local i=0 ok=1 note=""
  while [ "$i" -lt 20 ]; do
    if helper menuitems "$pid" Format | grep -qx "$other"; then ok=0; break; fi
    sleep 0.025; i=$((i + 1))
  done
  local ms=$(( $(now_ms) - t0 ))
  if [ "$ok" != 0 ] && wins "$record" | grep -q '"level":"sheet"'; then
    # TextEdit asked whether to convert: the press reached the guest. The
    # answer, then the change.
    helper key 36 # Return
    note=" (TextEdit asked to confirm the conversion)"
    t0=$(now_ms); i=0
    while [ "$i" -lt 80 ]; do
      if helper menuitems "$pid" Format | grep -qx "$other"; then ok=0; break; fi
      sleep 0.025; i=$((i + 1))
    done
    ms=$(( $(now_ms) - t0 ))
  fi
  [ "$ok" = 0 ] || { fail press "the Format menu did not show $other within 500 ms"; return; }
  [ "$ms" -le 500 ] || { fail press "the Format menu changed after $ms ms (limit 500)$note"; return; }
  wait_count "$events" "\"menu press Format > $target" "$presses" 2 || { fail press "no 'menu press Format > $target' line in events.jsonl"; return; }
  # A stale path is dropped, not sent: the dropped line, and no new
  # `menu press TITLES` line.
  local dropped; dropped=$(count_lines "$events" '"menu press dropped')
  local sent; sent=$(count_lines "$events" '"menu press [^d]')
  control "$record" "window menu-press 2.99" | grep -q '^ok' || { fail press "window menu-press refused"; return; }
  wait_count "$events" '"menu press dropped' "$dropped" 2 || { fail press "no 'menu press dropped' line for a stale path"; return; }
  sleep 0.5
  [ "$(count_lines "$events" '"menu press [^d]')" -eq "$sent" ] || { fail press "a press went out for a stale path"; return; }
  [ -z "$note" ] || echo "  note:$note"
  pass press
}

check_switch() {
  session_up menu "$record" /System/Applications/TextEdit.app >"$tmp/up.log" 2>&1 || { fail switch "no session: $(cat "$tmp/up.log")"; return; }
  local key_a=$key pid_a; pid_a=$(shim_pid "$record")
  session_up menub "$record_b" /System/Applications/Calculator.app >"$tmp/upb.log" 2>&1 || { fail switch "no second session: $(cat "$tmp/upb.log")"; return; }
  local key_b=$key pid_b; pid_b=$(shim_pid "$record_b")
  control "$record" "window raise $key_a" >/dev/null
  sleep 0.5
  helper front | grep -q "^$pid_a .*Format" || { fail switch "TextEdit's menus are not in front: $(helper front)"; return; }
  # To the second session.
  control "$record_b" "window raise $key_b" >/dev/null
  local t0; t0=$(now_ms)
  local i=0 front="" ok=1
  while [ "$i" -lt 50 ]; do
    front=$(helper front)
    case $front in "$pid_b "*View*) case $front in *Format*) ;; *) ok=0; break;; esac;; esac
    sleep 0.01; i=$((i + 1))
  done
  local ms=$(( $(now_ms) - t0 ))
  [ "$ok" = 0 ] || { fail switch "Calculator's menus did not come in front: $front"; return; }
  [ "$ms" -le 100 ] || { fail switch "Calculator's menus came after $ms ms (limit 100)"; return; }
  # And back.
  control "$record" "window raise $key_a" >/dev/null
  t0=$(now_ms); i=0; ok=1
  while [ "$i" -lt 50 ]; do
    front=$(helper front)
    case $front in "$pid_a "*Format*) ok=0; break;; esac
    sleep 0.01; i=$((i + 1))
  done
  ms=$(( $(now_ms) - t0 ))
  [ "$ok" = 0 ] || { fail switch "TextEdit's menus did not come back: $front"; return; }
  [ "$ms" -le 100 ] || { fail switch "TextEdit's menus came back after $ms ms (limit 100)"; return; }
  pass switch
  paddock app stop menub >/dev/null 2>&1
}

check_away() {
  session_up menu "$record" /System/Applications/TextEdit.app >"$tmp/up.log" 2>&1 || { fail away "no session: $(cat "$tmp/up.log")"; return; }
  local pid; pid=$(shim_pid "$record")
  helper menubar "$pid" | grep -qx Format || { fail away "no Format menu before the restart"; return; }
  local hellos menus; hellos=$(count_lines "$events" '"kind":"hello"'); menus=$(count_lines "$events" '"menu of')
  control "$record" "bench agent-restart" | grep -q '^ok' || { fail away "bench agent-restart refused"; return; }
  # Until the new hello: the tree stays on every sample.
  local i=0 samples=0
  while [ "$i" -lt 300 ]; do
    helper menubar "$pid" | grep -qx Format || { fail away "the Format menu went away while the agent was down"; return; }
    [ "$(menu_shown "$record")" = tree ] || { fail away "window menu says $(menu_shown "$record") while the agent was down"; return; }
    samples=$((samples + 1))
    [ "$(count_lines "$events" '"kind":"hello"')" -gt "$hellos" ] && break
    sleep 0.1; i=$((i + 1))
  done
  [ "$i" -lt 300 ] || { fail away "no new hello within 30 s"; return; }
  [ "$samples" -ge 2 ] || echo "  note: the agent was back after $samples sample(s)"
  wait_count "$events" '"menu of' "$menus" 2 || { fail away "no 'menu of' line within 2 s of the new hello"; return; }
  helper menubar "$pid" | grep -qx Format || { fail away "no Format menu after the new tree"; return; }
  pass away
}

needs_golden() { case $1 in tree|press|switch|away) return 0;; *) return 1;; esac; }

run_check() {
  local name=$1
  if [ "$name" = build ]; then build; return; fi
  if needs_golden "$name" && ! golden_ready; then skip "$name" "no golden: set PADDOCK_MACVM_GOLDEN to a sealed golden"; return; fi
  # The helper is built before the clock of a check starts.
  helper front >/dev/null 2>&1 || { skip "$name" "the Swift helper does not build (see $tmp/swiftc.log)"; return; }
  "check_$name"
}

case $what in
  all) for c in build tree press switch away; do run_check "$c"; done ;;
  build|tree|press|switch|away) run_check "$what" ;;
  *) echo "unknown check $what"; exit 2 ;;
esac
exit $((fails > 0))
