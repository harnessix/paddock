#!/usr/bin/env bash
# Keyboard layouts, input methods, the Globe key, two displays, Spaces and
# the clipboard conversions of the macOS guest (part 1), and the
# Accessibility mirror of the guest window for VoiceOver (part 2), checked
# on a Mac.
#
# Run this script on an Apple silicon Mac. It builds the
# worktree, signs `paddock-vm` with its entitlement, starts one session of a
# scratch profile in a temporary data folder (never the real paddock
# data), launches TextEdit in the guest, and runs every check through the
# control socket of the shim, the session record, `paddock app status
# --json` and the event log. Each check prints `PASS name`, `FAIL name:
# reason` or `SKIP name: reason`; the script exits 0 when every check that
# ran passed.
#
# Usage:
#   packaging/macos/checks/macvm-input.sh build     # compile, clippy, tests
#   packaging/macos/checks/macvm-input.sh all       # build plus every check
#   packaging/macos/checks/macvm-input.sh pinyin    # one check by name
#
# What it needs: a sealed golden (`paddock macvm setup`) named by
# PADDOCK_MACVM_GOLDEN, the Xcode command line tools (`swift` runs the
# synthetic keys, reads the Text Input Sources and the Mission Control
# accessibility tree), Accessibility and Screen Recording granted to the
# terminal that runs it, and for `displays` a second screen (a real one
# or one made with `displayplacer`). The script enables the German layout
# and the Pinyin input method on the host for its run and disables again
# the ones that were not enabled before; the host's input source is put
# back at the end. The `spaces` check adds one desktop in Mission Control
# and does not remove it: remove it by hand afterwards (Mission Control,
# hover the desktop, the close button). Without the golden, only `build`
# runs and the other checks print SKIP.
#
# The lines of the control socket that the checks use (src/vm/bridge.rs):
#   window list                 the host windows as JSON: id, title, level,
#                               x y w h, number, key
#   window move ID X Y W H      `window.move` to the guest
#   window raise ID             raise and activate the host window
#   window ax                   the mirror state as JSON: window, elements,
#                               host_elements, watched, away, tree
#   bench launch PATH           `app.launch` in the guest
#   bench ax SECONDS            one `WINDOW TEXT` line per `ax.value` event
#
# What proves each check:
#   build      `cargo build`, `cargo clippy -D warnings` and `cargo test
#              macvm::input macvm::order macvm::crop macvm::clipboard
#              macvm::axmirror guest::` exit 0 on macOS.
#   german     with the host on `com.apple.keylayout.German` and a session
#              window key, a `keyDown` for key code 6 (the US `z` key)
#              reaches the document as `y` (the `bench ax` line).
#   pinyin     with the host on `com.apple.inputmethod.SCIM.ITABC` and a
#              session window key: the host's current source read by
#              `TISCopyCurrentKeyboardInputSource` is `com.apple.keylayout.ABC`,
#              the session record has `input_source` SCIM.ITABC, `paddock
#              app status --json` has `health.input_source` SCIM.ITABC, the
#              key codes of `n i h a o` show a window of level `popover` in
#              `window list` (the candidates), and a space gives the
#              document `你好`.
#   restore    after the Finder becomes key, the host's source is SCIM.ITABC
#              again within 200 ms (polled at 20 ms). Then with a session
#              window key again and the host on ABC, `kill -9` of the shim
#              leaves the host on ABC, and `paddock app list` prints the
#              `input source ... restored` line and the host is on
#              SCIM.ITABC.
#   globe      with two host sources and a session window key, a press and
#              release of the Fn key (key code 63) changes
#              `health.input_source` within 3 s and leaves the host's source
#              as it was; events.jsonl has `input.set`.
#   displays   with two screens: the start lines of the session name the
#              display as the union, with the two scales note when the
#              scales differ; `window move` of a window into the second
#              screen puts it there in `window list`, and a screenshot of
#              the window's center through `screencapture` is light (the
#              white TextEdit document), not black.
#   spaces     with a second desktop made through the Mission Control
#              accessibility tree and window B dragged to it, a switch to
#              that desktop (Control-Right) gives `raised after Space change`
#              for B in events.jsonl within 500 ms, and the switch back
#              gives it for A.
#   voiceover  (part 2) the script reads the Accessibility tree of the key
#              host window through the system AX API, as VoiceOver does
#              (VoiceOver itself is not driven): the first query starts
#              the mirror (`an Accessibility client reads window` in
#              events.jsonl), and within 6 s the window has an element
#              whose title or description is `Bold` (TextEdit's format
#              bar); `AXPress` on it through the host gives `ax press` in
#              events.jsonl, and within 2 s the element's value, read
#              back through the agent's next `ax.tree`, differs from the
#              one before the press; `window ax` says `watched`.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-input.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profile="$root/profiles.noindex/input/default"
events="$profile/logs/events.jsonl"
record="$root/sessions/input--default.json"
GERMAN=com.apple.keylayout.German
PINYIN=com.apple.inputmethod.SCIM.ITABC
ABC=com.apple.keylayout.ABC
original_source=""
enabled_by_script=""

cleanup() {
  paddock app stop input >/dev/null 2>&1
  sleep 2
  pgrep -f "paddock-vm app --profile $profile" | xargs -r kill -9 2>/dev/null
  if [ -n "$original_source" ]; then helper tis select "$original_source" >/dev/null 2>&1; fi
  for id in $enabled_by_script; do helper tis disable "$id" >/dev/null 2>&1; done
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

# The Swift helper: synthetic input, the Text Input Sources, the screens,
# pixels and Mission Control. `swift FILE ARGS` compiles and runs it.
cat >"$tmp/input.swift" <<'EOF'
import Foundation
import Carbon
import AppKit
import ApplicationServices
import ImageIO
let a = CommandLine.arguments
func post(_ e: CGEvent?) { e?.post(tap: .cghidEventTap); usleep(30_000) }
func sourceID(_ s: TISInputSource) -> String {
  guard let p = TISGetInputSourceProperty(s, kTISPropertyInputSourceID) else { return "" }
  return Unmanaged<CFString>.fromOpaque(p).takeUnretainedValue() as String
}
func find(_ id: String) -> TISInputSource? {
  let list = TISCreateInputSourceList(nil, true).takeRetainedValue() as! [TISInputSource]
  return list.first { sourceID($0) == id }
}
func enabledIDs() -> [String] {
  let list = TISCreateInputSourceList(nil, false).takeRetainedValue() as! [TISInputSource]
  return list.map(sourceID)
}
// AX helpers for Mission Control (the Dock process).
func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
  var v: AnyObject?
  return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
}
func children(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] }
func findAll(_ e: AXUIElement, depth: Int, _ f: (AXUIElement) -> Bool, _ out: inout [AXUIElement]) {
  if f(e) { out.append(e) }
  if depth == 0 { return }
  for c in children(e) { findAll(c, depth: depth - 1, f, &out) }
}
func ident(_ e: AXUIElement) -> String { (attr(e, kAXIdentifierAttribute) as? String) ?? "" }
func desc(_ e: AXUIElement) -> String { (attr(e, kAXDescriptionAttribute) as? String) ?? "" }
func title(_ e: AXUIElement) -> String { (attr(e, kAXTitleAttribute) as? String) ?? "" }
func center(_ e: AXUIElement) -> CGPoint? {
  guard let pv = attr(e, kAXPositionAttribute), let sv = attr(e, kAXSizeAttribute) else { return nil }
  var p = CGPoint.zero; var s = CGSize.zero
  AXValueGetValue(pv as! AXValue, .cgPoint, &p); AXValueGetValue(sv as! AXValue, .cgSize, &s)
  return CGPoint(x: p.x + s.width / 2, y: p.y + s.height / 2)
}
func dockElement() -> AXUIElement? {
  guard let dock = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.dock").first else { return nil }
  return AXUIElementCreateApplication(dock.processIdentifier)
}
func missionControl(_ open: Bool) {
  // Control-Up opens Mission Control, Escape closes it.
  if open {
    let d = CGEvent(keyboardEventSource: nil, virtualKey: 126, keyDown: true); d?.flags = .maskControl
    let u = CGEvent(keyboardEventSource: nil, virtualKey: 126, keyDown: false); u?.flags = .maskControl
    post(d); post(u); usleep(1_200_000)
  } else {
    post(CGEvent(keyboardEventSource: nil, virtualKey: 53, keyDown: true))
    post(CGEvent(keyboardEventSource: nil, virtualKey: 53, keyDown: false)); usleep(800_000)
  }
}
func drag(_ from: CGPoint, _ to: CGPoint) {
  post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: from, mouseButton: .left))
  post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: from, mouseButton: .left))
  usleep(300_000)
  for i in 1...20 {
    let t = CGFloat(i) / 20
    let p = CGPoint(x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t)
    post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseDragged, mouseCursorPosition: p, mouseButton: .left))
    usleep(40_000)
  }
  usleep(500_000)
  post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: to, mouseButton: .left))
}
switch a[1] {
case "key":
  // key CODE [cmd|ctrl|fn]: a key down and up, with the modifier when asked.
  let code = CGKeyCode(UInt16(a[2])!)
  let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)
  let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)
  if a.count > 3 {
    switch a[3] {
    case "cmd": down?.flags = .maskCommand; up?.flags = .maskCommand
    case "ctrl": down?.flags = .maskControl; up?.flags = .maskControl
    case "fn": down?.flags = .maskSecondaryFn; up?.flags = []
    default: break
    }
  }
  post(down); post(up)
case "tis":
  // tis current | list | enable ID | select ID | disable ID
  switch a[2] {
  case "current": print(sourceID(TISCopyCurrentKeyboardInputSource().takeRetainedValue()))
  case "list": for id in enabledIDs() { print(id) }
  case "enable": guard let s = find(a[3]) else { print("no such source"); exit(1) }; exit(TISEnableInputSource(s) == 0 ? 0 : 1)
  case "select": guard let s = find(a[3]) else { print("no such source"); exit(1) }; exit(TISSelectInputSource(s) == 0 ? 0 : 1)
  case "disable": guard let s = find(a[3]) else { print("no such source"); exit(1) }; exit(TISDisableInputSource(s) == 0 ? 0 : 1)
  default: print("unknown"); exit(2)
  }
case "screens":
  // screens: one line per screen, `X Y W H SCALE`, AppKit coordinates, main first.
  for s in NSScreen.screens { let f = s.frame; print("\(Int(f.origin.x)) \(Int(f.origin.y)) \(Int(f.size.width)) \(Int(f.size.height)) \(Int(s.backingScaleFactor))") }
case "pixel":
  // pixel FILE X Y: the RGB of one pixel of a PNG, as `R G B`.
  let url = URL(fileURLWithPath: a[2]) as CFURL
  guard let src = CGImageSourceCreateWithURL(url, nil), let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else { print("no image"); exit(1) }
  let x = Int(a[3])!, y = Int(a[4])!
  let cs = CGColorSpaceCreateDeviceRGB()
  var px = [UInt8](repeating: 0, count: 4)
  let ctx = CGContext(data: &px, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4, space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
  ctx.draw(img, in: CGRect(x: -x, y: -(img.height - 1 - y), width: img.width, height: img.height))
  print("\(px[0]) \(px[1]) \(px[2])")
case "axfind", "axpress":
  // axfind PID LABEL: the role and the value (tab separated) of the first
  // element of the process's windows with that title or description,
  // through the system AX API. axpress PID LABEL: AXPress on it.
  func findAX(_ e: AXUIElement, _ depth: Int, _ f: (AXUIElement) -> Bool) -> AXUIElement? {
    if f(e) { return e }
    if depth == 0 { return nil }
    for c in children(e) { if let r = findAX(c, depth - 1, f) { return r } }
    return nil
  }
  func value(_ e: AXUIElement) -> String {
    guard let v = attr(e, kAXValueAttribute) else { return "" }
    if let s = v as? String { return s }
    if let n = v as? NSNumber { return n.stringValue }
    return "\(v)"
  }
  func role(_ e: AXUIElement) -> String { (attr(e, kAXRoleAttribute) as? String) ?? "" }
  let app = AXUIElementCreateApplication(pid_t(Int32(a[2])!))
  AXUIElementSetMessagingTimeout(app, 2.0)
  let wins = (attr(app, kAXWindowsAttribute) as? [AXUIElement]) ?? []
  var hit: AXUIElement? = nil
  for w in wins { if let h = findAX(w, 8, { title($0) == a[3] || desc($0) == a[3] }) { hit = h; break } }
  guard let el = hit else { exit(1) }
  if a[1] == "axfind" {
    print("\(role(el))\t\(value(el))")
  } else {
    let r = AXUIElementPerformAction(el, kAXPressAction as CFString)
    if r != .success { print("press failed: \(r.rawValue)"); exit(1) }
    print("pressed")
  }
case "mc":
  // mc spaces: the count of desktops. mc add: one more desktop.
  // mc move TITLE N: drag the window with that title to desktop N (1-based).
  guard let dock = dockElement() else { print("no Dock"); exit(1) }
  missionControl(true)
  defer { missionControl(false) }
  var lists: [AXUIElement] = []
  findAll(dock, depth: 8, { ident($0) == "mc.spaces.list" }, &lists)
  guard let spaces = lists.first else { print("no mc.spaces.list in the Dock AX tree"); exit(1) }
  let desktops = children(spaces).filter { desc($0).lowercased().hasPrefix("desktop") || title($0).lowercased().hasPrefix("desktop") }
  switch a[2] {
  case "spaces": print(desktops.count)
  case "add":
    var adds: [AXUIElement] = []
    findAll(dock, depth: 8, { ident($0) == "mc.spaces.add" || desc($0).lowercased().contains("add desktop") }, &adds)
    guard let add = adds.first else { print("no add desktop button"); exit(1) }
    if AXUIElementPerformAction(add, kAXPressAction as CFString) != .success { print("press failed"); exit(1) }
    usleep(800_000)
    print("added")
  case "move":
    let n = Int(a[4])! - 1
    guard n < desktops.count, let target = center(desktops[n]) else { print("no desktop \(a[4])"); exit(1) }
    var wins: [AXUIElement] = []
    findAll(dock, depth: 8, { title($0) == a[3] || desc($0) == a[3] }, &wins)
    guard let w = wins.first, let from = center(w) else { print("no window \(a[3]) in Mission Control"); exit(1) }
    drag(from, target)
    usleep(500_000)
    print("moved")
  default: print("unknown"); exit(2)
  }
default:
  print("unknown"); exit(2)
}
EOF
helper() { swift "$tmp/input.swift" "$@"; }

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
  for t in macvm::input macvm::order macvm::crop macvm::clipboard macvm::axmirror guest::; do
    if ! cargo test "$t" >>"$tmp/test.log" 2>&1; then
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

# control LINE: one line to the control socket of the session, the answer
# on stdout.
control() {
  local sock
  sock=$(sed -n 's/.*"control": *"\([^"]*\)".*/\1/p' "$record" | head -1)
  [ -n "$sock" ] || { echo "error no control socket in $record"; return 1; }
  printf '%s\n' "$1" | nc -U "$sock"
}
wins() { control "window list" | sed -n 's/^ok //p'; }
field() { printf '%s' "$1" | tr '{' '\n' | sed -n "s/.*\"$2\":\([^,}]*\).*/\1/p" | tr -d '"'; }
shim_pid() { sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$record" | head -1; }
# record_field NAME: a string field of the session record.
record_field() { tr -d '\n' <"$record" | sed -n "s/.*\"$1\": *\"\([^\"]*\)\".*/\1/p" | head -1; }
# guest_source: `health.input_source` of `paddock app status --json`.
guest_source() { paddock app status input --json 2>/dev/null | tr -d '\n ' | sed -n 's/.*"input_source":"\([^"]*\)".*/\1/p' | head -1; }
# wait_event KIND TEXT SECONDS
wait_event() {
  local i=0 n; n=$(awk "BEGIN{print int($3 * 10)}")
  while [ "$i" -lt "$n" ]; do
    grep -q "\"kind\":\"$1\".*$2" "$events" 2>/dev/null && return 0
    sleep 0.1; i=$((i + 1))
  done
  return 1
}
count_lines() { grep -c "$2" "$1" 2>/dev/null || echo 0; }
focus() { osascript -e "tell application \"$1\" to activate" >/dev/null 2>&1; sleep 0.5; }

# host_source_prepare: remember the host's source, enable German and
# Pinyin for the run.
host_source_prepare() {
  if [ -n "$original_source" ]; then return 0; fi
  original_source=$(helper tis current)
  local before; before=$(helper tis list)
  for id in $GERMAN $PINYIN $ABC; do
    if ! printf '%s\n' "$before" | grep -qx "$id"; then
      helper tis enable "$id" >/dev/null || return 1
      enabled_by_script="$enabled_by_script $id"
    fi
  done
  return 0
}

# The session with TextEdit and two documents. Sets $key and $other.
session_up() {
  if [ -f "$record" ] && kill -0 "$(shim_pid)" 2>/dev/null; then return 0; fi
  rm -f "$record"
  paddock app run input --backend macvm --network none >"$tmp/input.out" 2>&1 &
  wait_event hello agent 90 || { echo "no hello within 90 s (see $profile/logs/)"; return 1; }
  control "bench launch /System/Applications/TextEdit.app" | grep -q '^ok' || { echo "bench launch refused"; return 1; }
  local i=0
  while [ "$i" -lt 150 ]; do
    [ "$(field "$(wins)" id | wc -l)" -ge 1 ] && break
    sleep 0.1; i=$((i + 1))
  done
  local first; first=$(field "$(wins)" id | head -1)
  [ -n "$first" ] || { echo "TextEdit showed no window in 15 s"; return 1; }
  control "window raise $first" >/dev/null
  sleep 0.5
  helper key 45 cmd # Cmd-N: a second document
  i=0
  while [ "$i" -lt 50 ]; do
    [ "$(field "$(wins)" id | wc -l)" -ge 2 ] && break
    sleep 0.1; i=$((i + 1))
  done
  key=$(wins | tr '{' '\n' | grep '"key":true' | sed -n 's/.*"id":\([0-9]*\)[,}].*/\1/p' | head -1)
  other=$(field "$(wins)" id | grep -v "^$key$" | head -1)
  [ -n "$key" ] && [ -n "$other" ] || { echo "no two windows with a key one: $(wins)"; return 1; }
  return 0
}

# key_window: raise $key and wait for the input switch to settle.
key_window() { control "window raise $key" >/dev/null; sleep 0.4; }

# type_codes CODES...: key downs for the US key codes, 60 ms apart.
type_codes() { for code in "$@"; do helper key "$code"; sleep 0.06; done; }

# ax_lines SECONDS: the `bench ax` lines into $tmp/ax.lines, in the background.
ax_start() { control "bench ax $1" >"$tmp/ax.lines" 2>&1 & ax_pid=$!; sleep 0.8; }
ax_wait() { wait "$ax_pid" 2>/dev/null; }

check_german() {
  helper tis select $GERMAN || { fail german "cannot select $GERMAN on the host"; return; }
  sleep 0.3
  key_window
  ax_start 3
  type_codes 6 # the US `z` key: `y` on the German layout
  ax_wait
  if grep -q "^$key y" "$tmp/ax.lines"; then pass german; else fail german "the document did not get y: $(tail -3 "$tmp/ax.lines" | tr '\n' ' ')"; fi
}

check_pinyin() {
  focus Finder
  helper tis select $PINYIN || { fail pinyin "cannot select $PINYIN on the host"; return; }
  sleep 0.3
  key_window
  local host; host=$(helper tis current)
  [ "$host" = "$ABC" ] || { fail pinyin "the host is on $host while key, not $ABC"; return; }
  [ "$(record_field input_source)" = "$PINYIN" ] || { fail pinyin "the record has input_source '$(record_field input_source)'"; return; }
  local i=0 guest=""
  while [ "$i" -lt 50 ]; do
    guest=$(guest_source)
    [ "$guest" = "$PINYIN" ] && break
    sleep 0.1; i=$((i + 1))
  done
  [ "$guest" = "$PINYIN" ] || { fail pinyin "health.input_source is '$guest' after 5 s"; return; }
  ax_start 5
  type_codes 45 34 4 0 31 # n i h a o
  sleep 0.4
  local list; list=$(wins)
  local popover; popover=$(field "$list" level | grep -c popover)
  type_codes 49 # space: the first candidate
  ax_wait
  [ "$popover" -ge 1 ] || { fail pinyin "no popover window while composing: $list"; return; }
  if grep -q "^$key 你好" "$tmp/ax.lines"; then pass pinyin; else fail pinyin "the document did not get 你好: $(tail -3 "$tmp/ax.lines" | tr '\n' ' ')"; fi
}

check_restore() {
  helper tis select $PINYIN >/dev/null
  sleep 0.3
  key_window
  [ "$(helper tis current)" = "$ABC" ] || { fail restore "the host is not on $ABC while key"; return; }
  focus Finder
  local i=0 back=""
  while [ "$i" -lt 10 ]; do
    back=$(helper tis current)
    [ "$back" = "$PINYIN" ] && break
    sleep 0.02; i=$((i + 1))
  done
  [ "$back" = "$PINYIN" ] || { fail restore "the host is on $back 200 ms after the resign"; return; }
  # The crash: the shim dies while key, the host is left on ABC, and
  # `paddock app list` puts Pinyin back.
  key_window
  [ "$(helper tis current)" = "$ABC" ] || { fail restore "the host is not on $ABC before the kill"; return; }
  local pid; pid=$(shim_pid)
  kill -9 "$pid"
  sleep 1
  [ "$(helper tis current)" = "$ABC" ] || { fail restore "the host source changed after kill -9 without paddock"; return; }
  local out; out=$(paddock app list 2>&1)
  printf '%s\n' "$out" | grep -q "input source .* restored" || { fail restore "paddock app list did not print the restore line: $out"; return; }
  [ "$(helper tis current)" = "$PINYIN" ] || { fail restore "the host is on $(helper tis current) after paddock app list"; return; }
  pass restore
  session_up >"$tmp/up.log" 2>&1 || echo "note: no session after the restore check: $(cat "$tmp/up.log")"
}

check_globe() {
  helper tis select $GERMAN >/dev/null
  sleep 0.3
  key_window
  local before host_before; before=$(guest_source); host_before=$(helper tis current)
  local n; n=$(count_lines "$events" '"input.set')
  helper key 63 fn
  local i=0 after=""
  while [ "$i" -lt 30 ]; do
    after=$(guest_source)
    [ -n "$after" ] && [ "$after" != "$before" ] && break
    sleep 0.1; i=$((i + 1))
  done
  [ "$after" != "$before" ] || { fail globe "health.input_source stayed $before 3 s after the Globe key"; return; }
  [ "$(helper tis current)" = "$host_before" ] || { fail globe "the host's source changed to $(helper tis current)"; return; }
  [ "$(count_lines "$events" '"input.set')" -gt "$n" ] || { fail globe "no input.set line in events.jsonl"; return; }
  pass globe
}

check_displays() {
  local screens; screens=$(helper screens)
  [ "$(printf '%s\n' "$screens" | wc -l)" -ge 2 ] || { skip displays "one screen (add one with displayplacer)"; return; }
  local second; second=$(printf '%s\n' "$screens" | sed -n 2p)
  local sx sy sw sh; read -r sx sy sw sh _ <<<"$second"
  local main; main=$(printf '%s\n' "$screens" | sed -n 1p)
  local mx my mw mh ms; read -r mx my mw mh ms <<<"$main"
  local ss; ss=$(printf '%s\n' "$second" | awk '{print $5}')
  # The start lines: the display is the union, and the note when mixed.
  grep -q "^display " "$tmp/input.out" || { fail displays "no display line in the start lines"; return; }
  if [ "$ms" != "$ss" ] && ! grep -q "two scales" "$tmp/input.out"; then
    fail displays "two scales but no note in the start lines: $(grep '^display' "$tmp/input.out")"; return
  fi
  # Guest coordinates: the union's top left is the origin, y down. The
  # union's top is the highest screen top in AppKit coordinates.
  local ux uy_top; ux=$(printf '%s\n' "$screens" | awk 'NR==1{m=$1} $1<m{m=$1} END{print m}')
  uy_top=$(printf '%s\n' "$screens" | awk 'NR==1{m=$2+$4} $2+$4>m{m=$2+$4} END{print m}')
  local gx gy; gx=$((sx - ux + 100)); gy=$((uy_top - (sy + sh) + 100))
  control "window move $other $gx $gy 500 300" | grep -q '^ok' || { fail displays "window move refused"; return; }
  sleep 1
  local entry; entry=$(wins | tr '{' '\n' | grep "\"id\":$other[,}]")
  local x; x=$(field "$entry" x | cut -d. -f1)
  [ "${x:-0}" -ge "$((sx - ux))" ] || { fail displays "the window is at x $x, not on the second screen (from $((sx - ux)))"; return; }
  control "window raise $other" >/dev/null
  sleep 0.8
  # A pixel at the center of the window, in CoreGraphics coordinates
  # (origin top left of the main screen, y down).
  local cx cy; cx=$((sx + 100 + 250)); cy=$(( (my + mh) - (sy + sh) + 100 + 150 ))
  screencapture -x -R "$cx,$cy,2,2" "$tmp/shot.png" 2>/dev/null || { fail displays "screencapture failed (Screen Recording for the terminal?)"; return; }
  local rgb; rgb=$(helper pixel "$tmp/shot.png" 0 0)
  local r g b; read -r r g b <<<"$rgb"
  if [ "${r:-0}" -gt 180 ] && [ "${g:-0}" -gt 180 ] && [ "${b:-0}" -gt 180 ]; then
    pass displays
  else
    fail displays "the pixel at the window's center is $rgb, not the white document"
  fi
}

check_spaces() {
  local n; n=$(helper mc spaces 2>&1) || { skip spaces "Mission Control AX: $n"; return; }
  if [ "${n:-0}" -lt 2 ]; then
    helper mc add >/dev/null 2>&1 || { skip spaces "cannot add a desktop through Mission Control"; return; }
  fi
  local title; title=$(wins | tr '{' '\n' | grep "\"id\":$other[,}]" | sed -n 's/.*"title":"\([^"]*\)".*/\1/p')
  [ -n "$title" ] || { fail spaces "window $other has no title"; return; }
  local out; out=$(helper mc move "$title" 2 2>&1) || { skip spaces "cannot move the window in Mission Control: $out"; return; }
  sleep 1
  local before; before=$(count_lines "$events" "raised after Space change")
  helper key 124 ctrl # Control-Right: the next desktop
  local i=0 ok=1
  while [ "$i" -lt 5 ]; do
    if [ "$(count_lines "$events" "raised after Space change")" -gt "$before" ]; then ok=0; break; fi
    sleep 0.1; i=$((i + 1))
  done
  [ "$ok" = 0 ] || { helper key 123 ctrl; fail spaces "no raise within 500 ms of the switch"; return; }
  local line; line=$(grep "raised after Space change" "$events" | tail -1)
  printf '%s' "$line" | grep -q "\"id\":$other[,.}]" || { helper key 123 ctrl; fail spaces "the raise names another window: $line"; return; }
  before=$(count_lines "$events" "raised after Space change")
  helper key 123 ctrl # back to the first desktop
  sleep 0.6
  line=$(grep "raised after Space change" "$events" | tail -1)
  [ "$(count_lines "$events" "raised after Space change")" -gt "$before" ] && printf '%s' "$line" | grep -q "\"id\":$key[,.}]" || { fail spaces "the switch back did not raise $key: $line"; return; }
  pass spaces
}

check_voiceover() {
  key_window
  local pid; pid=$(shim_pid)
  # The first query of the host window's children through the AX API
  # starts the mirror; the tree is there on a later query.
  local i=0 found=""
  while [ "$i" -lt 60 ]; do
    found=$(helper axfind "$pid" Bold 2>/dev/null)
    [ -n "$found" ] && break
    sleep 0.1; i=$((i + 1))
  done
  [ -n "$found" ] || { fail voiceover "no element Bold in the AX tree of the host window within 6 s (window ax: $(control 'window ax' | head -c 400))"; return; }
  grep -q '"an Accessibility client reads window' "$events" || { fail voiceover "no 'an Accessibility client reads window' line in events.jsonl"; return; }
  local role before; role=$(printf '%s' "$found" | cut -f1); before=$(printf '%s' "$found" | cut -f2)
  echo "  note: Bold is a $role with the value '$before'"
  local presses; presses=$(count_lines "$events" '"ax press ')
  local out; out=$(helper axpress "$pid" Bold 2>&1) || { fail voiceover "AXPress on Bold failed: $out"; return; }
  wait_count "$events" '"ax press ' "$presses" 2 || { fail voiceover "no 'ax press' line in events.jsonl within 2 s"; return; }
  # The state toggles in the guest: the value read back through the
  # agent's next tree differs.
  i=0; local after="$before"
  while [ "$i" -lt 40 ]; do
    after=$(helper axfind "$pid" Bold 2>/dev/null | cut -f2)
    [ "$after" != "$before" ] && break
    sleep 0.05; i=$((i + 1))
  done
  [ "$after" != "$before" ] || { fail voiceover "the value of Bold stayed '$before' 2 s after AXPress (the guest did not toggle, or Bold has no value: see window ax)"; return; }
  control "window ax" | grep -q '"watched":true' || { fail voiceover "window ax does not say watched: $(control 'window ax' | head -c 200)"; return; }
  # A second press puts it back, so the document is as before.
  helper axpress "$pid" Bold >/dev/null 2>&1
  pass voiceover
}

needs_session() { case $1 in german|pinyin|restore|globe|displays|spaces|voiceover) return 0;; *) return 1;; esac; }

run_check() {
  local name=$1
  if [ "$name" = build ]; then build; return; fi
  if ! golden_ready; then skip "$name" "no golden: set PADDOCK_MACVM_GOLDEN to a sealed golden"; return; fi
  if needs_session "$name"; then
    host_source_prepare || { skip "$name" "cannot enable the input sources on the host"; return; }
    session_up >"$tmp/up.log" 2>&1 || { fail "$name" "no session: $(cat "$tmp/up.log")"; return; }
  fi
  "check_$name"
}

case $what in
  all) for c in build german pinyin globe displays spaces voiceover restore; do run_check "$c"; done ;;
  build|german|pinyin|restore|globe|displays|spaces|voiceover) run_check "$what" ;;
  *) echo "unknown check $what"; exit 2 ;;
esac
exit $((fails > 0))
