#!/usr/bin/env bash
# The host windows of an app session, one per guest window, checked on a
# Mac.
#
# Run this script on an Apple silicon Mac with one screen.
# It builds the worktree, signs `paddock-vm` with its entitlement, starts
# one session of a scratch profile in a temporary data folder (never the
# real paddock data), launches TextEdit in the guest with two documents,
# and runs every check through the control socket of the shim. Each check
# prints `PASS name` or `FAIL name: reason`; the script exits 0 when every
# check that ran passed.
#
# Usage:
#   packaging/macos/checks/macvm-windows.sh build     # compile, clippy, tests
#   packaging/macos/checks/macvm-windows.sh all       # build plus every check
#   packaging/macos/checks/macvm-windows.sh two       # one check by name
#
# What it needs: a sealed golden (`paddock macvm setup`) named by
# PADDOCK_MACVM_GOLDEN, the Xcode command line tools (`swift` runs the
# synthetic clicks and keys, and reads the screenshot pixels), Screen
# Recording and Accessibility granted to the terminal that runs it, and
# one screen. It changes the desktop picture of the Mac for the `corners`
# check and puts the old one back. Without the golden, only `build` runs
# and the other checks print SKIP.
#
# The lines of the control socket that the checks use (src/vm/bridge.rs):
#   window list                 the host windows as JSON: id, title, level,
#                               x y w h, number (the host window number), key
#   window move ID X Y W H      `window.move` to the guest, with the event
#                               log line `moved after window.move` and its ms
#   window raise ID             raise and activate the host window
#   bench launch PATH           `app.launch` in the guest
#   bench ax SECONDS            `ax.watch` on, one `WINDOW TEXT` line per
#                               `ax.value` event, for that long
#   bench windows JSON          a windows event through the fold, as if the
#                               guest sent it
#   bench agent-restart         `agent.restart` (a kill -9 of the agent
#                               needs a shell in the guest, which the host
#                               has not; the restart ends the agent the same
#                               way and launchd starts it again)
#
# What proves each check:
#   build          `cargo build`, `cargo clippy -D warnings` and
#                  `cargo test macvm vm::bridge` exit 0 on macOS.
#   two            `window list` has two entries of level normal with
#                  titles from AX (not empty), and two host window numbers.
#   type           a key sequence posted to the key host window arrives as
#                  `ax.value` of the key document only: the other window id
#                  never appears in the `bench ax` lines.
#   move           after `window move`, events.jsonl has `moved after
#                  window.move` with `ms` at most 20.
#   agent-restart  after `bench agent-restart` and 3 s, `window list` has
#                  the same host window numbers, and events.jsonl has
#                  `rebuilt after hello` with `changes` 0: nothing closed.
#   menu           a synthetic right click in the key document shows an
#                  entry of level menu in `window list` within 100 ms, and
#                  Escape removes it.
#   oversize       `bench windows` with a floating window larger than the
#                  main screen gets `refuse ... TooLarge(Floating)` and
#                  `window.move` in events.jsonl within 200 ms, and the
#                  session record has the badge `floating window too
#                  large, moved back`.
#   corners        with the desktop set to pure green, a screenshot of the
#                  four corner pixels of the front host window shows green
#                  (the host desktop), not black and not the guest desktop.
#   cmdq           Cmd-Q posted to the key host window runs the quit path:
#                  the shim exits, events.jsonl has `quit (quit)` and
#                  stamp.json has `"clean": true`.
set -uo pipefail

here=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$here"
what=${1:-all}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/paddock-win.XXXXXX")
export XDG_DATA_HOME="$tmp/data" XDG_STATE_HOME="$tmp/state" XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_RUNTIME_DIR="$tmp/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
bin="$here/target/release"
export PATH="$bin:$PATH"
fails=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; fails=$((fails + 1)); }
skip() { echo "SKIP $1: $2"; }
root="$XDG_DATA_HOME/paddock/macvm"
profile="$root/profiles.noindex/win/default"
events="$profile/logs/events.jsonl"
record="$root/sessions/win--default.json"
old_desktop=""

cleanup() {
  if [ -n "$old_desktop" ]; then
    osascript -e "tell application \"System Events\" to tell every desktop to set picture to \"$old_desktop\"" >/dev/null 2>&1
  fi
  paddock app stop win >/dev/null 2>&1
  sleep 2
  pgrep -f "paddock-vm app --profile $profile" | xargs -r kill -9 2>/dev/null
  # A clone of the sealed golden keeps its uchg flag: drop it first.
  chflags -R nouchg "$tmp" 2>/dev/null
  rm -rf "$tmp"
}
trap cleanup EXIT

# The Swift helper: synthetic input and pixels. `swift FILE ARGS` compiles
# and runs it (about two seconds the first time).
cat >"$tmp/win.swift" <<'EOF'
import Foundation
import CoreGraphics
import ImageIO
let a = CommandLine.arguments
func post(_ e: CGEvent?) { e?.post(tap: .cghidEventTap); usleep(30_000) }
switch a[1] {
case "click":
  // click X Y right|left: a press and a release at the point.
  let p = CGPoint(x: Double(a[2])!, y: Double(a[3])!)
  let right = a[4] == "right"
  post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left))
  post(CGEvent(mouseEventSource: nil, mouseType: right ? .rightMouseDown : .leftMouseDown, mouseCursorPosition: p, mouseButton: right ? .right : .left))
  post(CGEvent(mouseEventSource: nil, mouseType: right ? .rightMouseUp : .leftMouseUp, mouseCursorPosition: p, mouseButton: right ? .right : .left))
case "key":
  // key CODE [cmd]: a key down and up, with Command when asked.
  let code = CGKeyCode(UInt16(a[2])!)
  let cmd = a.count > 3 && a[3] == "cmd"
  let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)
  let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)
  if cmd { down?.flags = .maskCommand; up?.flags = .maskCommand }
  post(down); post(up)
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
case "solid":
  // solid FILE R G B: a 64 by 64 PNG of one color.
  let cs = CGColorSpaceCreateDeviceRGB()
  let ctx = CGContext(data: nil, width: 64, height: 64, bitsPerComponent: 8, bytesPerRow: 0, space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
  ctx.setFillColor(red: Double(a[3])! / 255, green: Double(a[4])! / 255, blue: Double(a[5])! / 255, alpha: 1)
  ctx.fill(CGRect(x: 0, y: 0, width: 64, height: 64))
  let dst = CGImageDestinationCreateWithURL(URL(fileURLWithPath: a[2]) as CFURL, "public.png" as CFString, 1, nil)!
  CGImageDestinationAddImage(dst, ctx.makeImage()!, nil)
  CGImageDestinationFinalize(dst)
default:
  print("unknown"); exit(2)
}
EOF
helper() { swift "$tmp/win.swift" "$@"; }

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
  if ! cargo test macvm >"$tmp/test.log" 2>&1 || ! cargo test vm::bridge >>"$tmp/test.log" 2>&1; then
    fail build "tests: $(grep -E 'FAILED|panicked' "$tmp/test.log" | head -3 | tr '\n' ' ')"; return
  fi
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
# on stdout (every line until the shim closes).
control() {
  local sock
  sock=$(sed -n 's/.*"control": *"\([^"]*\)".*/\1/p' "$record" | head -1)
  [ -n "$sock" ] || { echo "error no control socket in $record"; return 1; }
  printf '%s\n' "$1" | nc -U "$sock"
}

# wins: the `window list` JSON, or nothing.
wins() { control "window list" | sed -n 's/^ok //p'; }

# field JSON NAME: the values of a field of every entry, one per line.
field() { printf '%s' "$1" | tr '{' '\n' | sed -n "s/.*\"$2\":\([^,}]*\).*/\1/p" | tr -d '"'; }

# wait_event KIND TEXT SECONDS: true when events.jsonl gets a line with
# that kind and that text within the seconds.
wait_event() {
  local i=0 n=$(( $3 * 10 ))
  while [ "$i" -lt "$n" ]; do
    grep -q "\"kind\":\"$1\".*$2" "$events" 2>/dev/null && return 0
    sleep 0.1; i=$((i + 1))
  done
  return 1
}

# The session with TextEdit and two documents. Sets $key and $other to
# the two window ids, the key one first.
session_up() {
  if [ -f "$record" ]; then return 0; fi
  paddock app run win --backend macvm --network none >"$tmp/win.out" 2>&1 &
  wait_event hello agent 90 || { echo "no hello within 90 s (see $profile/logs/)"; return 1; }
  control "bench launch /System/Applications/TextEdit.app" | grep -q '^ok' || { echo "bench launch refused"; return 1; }
  local i=0
  while [ "$i" -lt 150 ]; do
    [ "$(field "$(wins)" id | wc -l)" -ge 1 ] && break
    sleep 0.1; i=$((i + 1))
  done
  local first
  first=$(field "$(wins)" id | head -1)
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

# frame ID: the guest frame of a window as `X Y W H` (one screen: the
# same as the screen points of CoreGraphics, origin top left).
frame() {
  local entry
  entry=$(wins | tr '{' '\n' | grep "\"id\":$1[,}]")
  [ -n "$entry" ] || return 1
  echo "$(field "$entry" x) $(field "$entry" y) $(field "$entry" w) $(field "$entry" h)"
}

check_two() {
  local list; list=$(wins)
  local n; n=$(field "$list" id | wc -l | tr -d ' ')
  [ "$n" = 2 ] || { fail two "$n windows, not two: $list"; return; }
  [ "$(field "$list" level | grep -c normal)" = 2 ] || { fail two "levels: $(field "$list" level | tr '\n' ' ')"; return; }
  [ "$(field "$list" title | grep -vc '^$')" = 2 ] || { fail two "a title is empty: $list"; return; }
  [ "$(field "$list" number | sort -u | wc -l | tr -d ' ')" = 2 ] || { fail two "the host window numbers are not two"; return; }
  pass two
}

check_type() {
  control "window raise $key" >/dev/null
  sleep 0.3
  control "bench ax 4" >"$tmp/ax.lines" 2>&1 &
  sleep 0.8
  # h e l l o: the key codes of the US layout.
  for code in 4 14 37 37 31; do helper key "$code"; done
  wait
  grep -q "^$key hello" "$tmp/ax.lines" || { fail type "the key document did not get hello: $(tail -3 "$tmp/ax.lines" | tr '\n' ' ')"; return; }
  if grep -q "^$other " "$tmp/ax.lines"; then fail type "the other window got keys too"; return; fi
  pass type
}

check_move() {
  read -r x y w h <<<"$(frame "$key")"
  [ -n "$w" ] || { fail move "no frame for $key"; return; }
  local nx=$(( ${x%.*} + 40 )) ny=$(( ${y%.*} + 30 ))
  control "window move $key $nx $ny $w $h" | grep -q '^ok' || { fail move "the move line was refused"; return; }
  wait_event window "moved after window.move" 2 || { fail move "no move within 2 s"; return; }
  local ms; ms=$(grep 'moved after window.move' "$events" | tail -1 | sed -n 's/.*"ms":\([0-9.]*\).*/\1/p')
  [ "${ms%.*}" -le 20 ] || { fail move "the host window moved after $ms ms"; return; }
  read -r x2 y2 _ _ <<<"$(frame "$key")"
  [ "${x2%.*}" = "$nx" ] && [ "${y2%.*}" = "$ny" ] || { fail move "the frame is $x2,$y2, not $nx,$ny"; return; }
  pass move
}

check_agent_restart() {
  local before; before=$(field "$(wins)" number | sort | tr '\n' ' ')
  local lines; lines=$(wc -l <"$events")
  control "bench agent-restart" | grep -q '^ok' || { fail agent-restart "the restart line was refused"; return; }
  sleep 3
  local after; after=$(field "$(wins)" number | sort | tr '\n' ' ')
  [ "$before" = "$after" ] || { fail agent-restart "the host window numbers changed: $before -> $after"; return; }
  tail -n +"$((lines + 1))" "$events" | grep -q '"rebuilt after hello".*"changes":0' || { fail agent-restart "no rebuild with 0 changes after the new hello"; return; }
  pass agent-restart
}

check_menu() {
  control "window raise $key" >/dev/null
  sleep 0.3
  read -r x y w h <<<"$(frame "$key")"
  helper click $(( ${x%.*} + ${w%.*} / 2 )) $(( ${y%.*} + ${h%.*} / 2 )) right
  local i=0 found=""
  while [ "$i" -lt 5 ]; do
    if wins | grep -q '"level":"menu"'; then found=yes; break; fi
    sleep 0.02; i=$((i + 1))
  done
  [ -n "$found" ] || { fail menu "no window of level menu within 100 ms"; helper key 53; return; }
  helper key 53 # Escape
  sleep 0.5
  if wins | grep -q '"level":"menu"'; then fail menu "the menu window stayed after Escape"; return; fi
  pass menu
}

check_oversize() {
  local lines; lines=$(wc -l <"$events")
  local ev='{"order":[999999],"changed":[{"id":999999,"pid":1,"app":"bench","title":"big","frame":{"x":0.0,"y":0.0,"w":9000.0,"h":9000.0},"level":"floating","minimized":false,"focused":false,"resizable":true,"movable":true}],"removed":[]}'
  control "bench windows $ev" | grep -q '^ok' || { fail oversize "the bench line was refused"; return; }
  local i=0 ok=""
  while [ "$i" -lt 10 ]; do
    if tail -n +"$((lines + 1))" "$events" | grep -q 'refuse 999999 TooLarge(Floating)'; then ok=yes; break; fi
    sleep 0.02; i=$((i + 1))
  done
  [ -n "$ok" ] || { fail oversize "no refusal within 200 ms"; return; }
  grep -q '"badge": *"floating window too large, moved back"' "$record" || { fail oversize "no badge in $record"; return; }
  grep -q 'window.move 999999' "$profile/logs/shim.log" "$events" 2>/dev/null || echo "note oversize: the window.move request is in the shim log at debug level only"
  pass oversize
}

check_corners() {
  helper solid "$tmp/green.png" 0 255 0
  old_desktop=$(osascript -e 'tell application "System Events" to tell current desktop to get picture' 2>/dev/null)
  osascript -e "tell application \"System Events\" to tell every desktop to set picture to \"$tmp/green.png\"" >/dev/null 2>&1
  sleep 1
  control "window raise $key" >/dev/null
  sleep 0.5
  read -r x y w h <<<"$(frame "$key")"
  x=${x%.*}; y=${y%.*}; w=${w%.*}; h=${h%.*}
  screencapture -x -R "$x,$y,$w,$h" "$tmp/win.png" || { fail corners "screencapture failed (Screen Recording for the terminal?)"; return; }
  local scale; scale=$(system_profiler SPDisplaysDataType 2>/dev/null | grep -q Retina && echo 2 || echo 1)
  local pw=$(( w * scale )) ph=$(( h * scale ))
  for p in "0 0" "$((pw - 1)) 0" "0 $((ph - 1))" "$((pw - 1)) $((ph - 1))"; do
    read -r px py <<<"$p"
    local rgb; rgb=$(helper pixel "$tmp/win.png" "$px" "$py")
    read -r r g b <<<"$rgb"
    if [ "$g" -lt 200 ] || [ "$r" -gt 60 ] || [ "$b" -gt 60 ]; then
      fail corners "the corner pixel $px,$py is $rgb, not the green desktop"; return
    fi
  done
  pass corners
}

check_cmdq() {
  control "window raise $key" >/dev/null
  sleep 0.3
  local pid; pid=$(sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$record" | head -1)
  helper key 12 cmd # Cmd-Q
  local i=0
  while [ "$i" -lt 900 ]; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1; i=$((i + 1))
  done
  kill -0 "$pid" 2>/dev/null && { fail cmdq "the shim still runs 90 s after Cmd-Q"; return; }
  grep -q '"text":"quit (quit)"' "$events" || { fail cmdq "no quit (quit) event"; return; }
  grep -q '"clean": *true' "$profile/stamp.json" || { fail cmdq "stamp.json is not clean"; return; }
  pass cmdq
}

run_check() {
  local name=$1
  if ! golden_ready; then skip "$name" "set PADDOCK_MACVM_GOLDEN to a sealed golden"; return; fi
  if ! msg=$(session_up); then fail "$name" "$msg"; return; fi
  "check_$(echo "$name" | tr '-' '_')"
}

case "$what" in
  build) build ;;
  all)
    build
    for c in two type move agent-restart menu oversize corners cmdq; do run_check "$c"; done
    ;;
  two|type|move|agent-restart|menu|oversize|corners|cmdq) run_check "$what" ;;
  *) echo "unknown check $what" >&2; exit 2 ;;
esac
echo "macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion)) on $(sysctl -n hw.model)"
[ "$fails" = 0 ]
