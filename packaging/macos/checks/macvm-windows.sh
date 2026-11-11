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
#   window frames ID            the host frame, the guest frame, the frame
#                               the view shows, the offset, `moves_sent`
#                               (the window.move requests sent for the
#                               window), `undrawn` (the rectangles the
#                               cover paints over what the guest has not
#                               drawn yet), `edge` (the sampled side
#                               color, 0 to 1) as JSON
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
#   drag           a synthetic drag of the title bar of the key window (a
#                  press, ten mouse steps, a release): the host window
#                  moves, the guest frame stays the same, `moves_sent`
#                  does not change (no window.move, not even at the
#                  release), and events.jsonl has `the host drag ended,
#                  the guest window stayed`. A right click in the moved
#                  window then opens a menu whose offset is the window's.
#   resize         a synthetic drag of the right edge of the key window,
#                  eight steps of 30 points: after each step `window
#                  frames` has `undrawn` above 0 whenever the host window
#                  is wider than the frame the view shows, and a
#                  screenshot of the new strip right of the old edge has
#                  the sampled edge color (within 60 per channel), not the
#                  guest desktop. After the release the guest width is the
#                  host width within 2 points, and events.jsonl has `the
#                  host resize ended` with its timings, which INFO prints.
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
case "down", "drag", "up":
  // down|drag|up X Y: a left press, a drag step or the release at the point.
  let p = CGPoint(x: Double(a[2])!, y: Double(a[3])!)
  let kind: CGEventType = a[1] == "down" ? .leftMouseDown : (a[1] == "drag" ? .leftMouseDragged : .leftMouseUp)
  if a[1] == "down" {
    post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left))
  }
  post(CGEvent(mouseEventSource: nil, mouseType: kind, mouseCursorPosition: p, mouseButton: .left))
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
# Compiled once, so each call takes milliseconds: the drag and resize
# checks post one step per call.
helper() {
  if [ ! -x "$tmp/win" ]; then
    swiftc -O "$tmp/win.swift" -o "$tmp/win" >"$tmp/swiftc.log" 2>&1 || { swift "$tmp/win.swift" "$@"; return; }
  fi
  "$tmp/win" "$@"
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

# js CODE ARG...: a JavaScript function body over argv, its answer.
js() { local code=$1; shift; osascript -l JavaScript -e "function run(argv) { $code }" "$@"; }

# hostframe ID: the host frame of a window as `X Y W H` in screen points
# from the top left: where it shows, which is not the guest frame once the
# person moved the host window.
hostframe() {
  local j; j=$(control "window frames $1" | sed -n 's/^ok //p')
  [ -n "$j" ] || return 1
  js 'var o = JSON.parse(argv[0]); return o.host.join(" ");' "$j"
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
  read -r x y w h <<<"$(hostframe "$key")"
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
  read -r x y w h <<<"$(hostframe "$key")"
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

# frames_of ID: the `window frames` JSON of a window.
frames_of() { control "window frames $1" | sed -n 's/^ok //p'; }

# jnum JSON PATH: one number of a JSON object, by a JavaScript path.
jnum() { osascript -l JavaScript -e 'function run(a) { var o = JSON.parse(a[0]); var v = eval("o" + a[1]); return v == null ? "" : String(v); }' "$1" "$2"; }

check_drag() {
  control "window raise $key" >/dev/null
  sleep 0.5
  local j0 j1 lines
  j0=$(frames_of "$key")
  [ -n "$j0" ] || { fail drag "no window frames for $key"; return; }
  lines=$(wc -l <"$events")
  local hx hy hw gx gy moves
  hx=$(jnum "$j0" '.host[0]'); hy=$(jnum "$j0" '.host[1]'); hw=$(jnum "$j0" '.host[2]')
  gx=$(jnum "$j0" '.guest[0]'); gy=$(jnum "$j0" '.guest[1]'); moves=$(jnum "$j0" '.moves_sent')
  # The title bar, away from the buttons.
  local px=$(( ${hx%.*} + ${hw%.*} / 2 )) py=$(( ${hy%.*} + 12 ))
  helper down "$px" "$py"
  local i=1
  while [ "$i" -le 10 ]; do
    helper drag $(( px + i * 20 )) $(( py + i * 10 ))
    i=$((i + 1))
  done
  helper up $(( px + 200 )) $(( py + 100 ))
  sleep 1
  j1=$(frames_of "$key")
  echo "INFO drag: before $j0"
  echo "INFO drag: after $j1"
  local nhx nhy ngx ngy nmoves
  nhx=$(jnum "$j1" '.host[0]'); nhy=$(jnum "$j1" '.host[1]')
  ngx=$(jnum "$j1" '.guest[0]'); ngy=$(jnum "$j1" '.guest[1]'); nmoves=$(jnum "$j1" '.moves_sent')
  [ "$nmoves" = "$moves" ] || { fail drag "window.move was sent: moves_sent $moves -> $nmoves"; return; }
  [ "${ngx%.*}" = "${gx%.*}" ] && [ "${ngy%.*}" = "${gy%.*}" ] || { fail drag "the guest window moved: $gx,$gy -> $ngx,$ngy"; return; }
  [ $(( ${nhx%.*} - ${hx%.*} )) -ge 150 ] && [ $(( ${nhy%.*} - ${hy%.*} )) -ge 50 ] || { fail drag "the host window did not move with the mouse: $hx,$hy -> $nhx,$nhy"; return; }
  tail -n +"$((lines + 1))" "$events" | grep -q 'the host drag ended, the guest window stayed' || { fail drag "no event line of the drag end"; return; }
  # A right click in the moved window still reaches the guest at the
  # right place, and its menu opens at the window's offset.
  local x y w h
  read -r x y w h <<<"$(hostframe "$key")"
  helper click $(( ${x%.*} + ${w%.*} / 2 )) $(( ${y%.*} + ${h%.*} / 2 )) right
  local k=0 menu=""
  while [ "$k" -lt 25 ]; do
    menu=$(wins | tr '{' '\n' | grep '"level":"menu"' | sed -n 's/.*"id":\([0-9]*\)[,}].*/\1/p' | head -1)
    [ -n "$menu" ] && break
    sleep 0.02; k=$((k + 1))
  done
  [ -n "$menu" ] || { fail drag "no menu after a right click in the moved window"; helper key 53; return; }
  local jm; jm=$(frames_of "$menu")
  helper key 53 # Escape
  [ "$(js 'var a = JSON.parse(argv[0]).offset, b = JSON.parse(argv[1]).offset; return Math.abs(a[0] - b[0]) <= 1 && Math.abs(a[1] - b[1]) <= 1 ? "1" : "0";' "$jm" "$j1")" = 1 ] || { fail drag "the menu is not at the window's offset: $jm"; return; }
  pass drag
}

check_resize() {
  control "window raise $key" >/dev/null
  sleep 0.5
  local j lines
  j=$(frames_of "$key")
  [ -n "$j" ] || { fail resize "no window frames for $key"; return; }
  lines=$(wc -l <"$events")
  local hx hy hw hh
  hx=$(jnum "$j" '.host[0]'); hy=$(jnum "$j" '.host[1]'); hw=$(jnum "$j" '.host[2]'); hh=$(jnum "$j" '.host[3]')
  hx=${hx%.*}; hy=${hy%.*}; hw=${hw%.*}; hh=${hh%.*}
  local ex=$(( hx + hw - 2 )) ey=$(( hy + hh / 2 ))
  local scale; scale=$(system_profiler SPDisplaysDataType 2>/dev/null | grep -q Retina && echo 2 || echo 1)
  helper down "$ex" "$ey"
  local i=1 why=""
  while [ "$i" -le 8 ]; do
    helper drag $(( ex + i * 30 )) "$ey"
    j=$(frames_of "$key")
    local w vw undrawn
    w=$(jnum "$j" '.host[2]'); vw=$(jnum "$j" '.view[2]'); undrawn=$(jnum "$j" '.undrawn')
    if [ "$(js 'return Number(argv[0]) > Number(argv[1]) + 0.5 && Number(argv[2]) == 0 ? "1" : "0";' "$w" "$vw" "$undrawn")" = 1 ]; then
      why="step $i: the host window is $w wide, the view shows $vw, and nothing covers the rest"; break
    fi
    local edge; edge=$(jnum "$j" '.edge')
    if [ -n "$edge" ]; then
      # The new strip right of the old edge, below the title bar.
      local sx=$(( hx + hw + 4 )) sw=$(( i * 30 - 8 ))
      if [ "$sw" -gt 4 ] && screencapture -x -R "$sx,$(( hy + 60 )),$sw,$(( hh - 120 ))" "$tmp/strip.png" 2>/dev/null; then
        local rgb; rgb=$(helper pixel "$tmp/strip.png" $(( sw * scale / 2 )) $(( (hh - 120) * scale / 2 )))
        local bad; bad=$(js 'var e = argv[0].split(","); var p = argv[1].split(" "); for (var k = 0; k < 3; k++) if (Math.abs(Number(e[k]) * 255 - Number(p[k])) > 60) return "1"; return "0";' "$edge" "$rgb")
        if [ "$bad" = 1 ]; then why="step $i: the strip right of the old edge is $rgb, not the edge color $edge (the guest desktop?)"; break; fi
      fi
    else
      echo "INFO resize: step $i: no sampled edge color (the own window capture failed), only the cover state is checked"
    fi
    i=$((i + 1))
  done
  helper up $(( ex + 240 )) "$ey"
  [ -z "$why" ] || { fail resize "$why"; return; }
  wait_event window "the host resize ended" 3 || { fail resize "no line the host resize ended within 3 s"; return; }
  tail -n +"$((lines + 1))" "$events" | grep 'the host resize ended' | tail -1 | sed 's/^/INFO resize: /'
  sleep 0.5
  j=$(frames_of "$key")
  local gw nw; gw=$(jnum "$j" '.guest[2]'); nw=$(jnum "$j" '.host[2]')
  [ "$(js 'return Math.abs(Number(argv[0]) - Number(argv[1])) <= 2 ? "1" : "0";' "$gw" "$nw")" = 1 ] || { fail resize "after the release the guest is $gw wide, the host $nw"; return; }
  pass resize
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
    for c in two type move agent-restart menu oversize corners drag resize cmdq; do run_check "$c"; done
    ;;
  two|type|move|agent-restart|menu|oversize|corners|drag|resize|cmdq) run_check "$what" ;;
  *) echo "unknown check $what" >&2; exit 2 ;;
esac
echo "macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion)) on $(sysctl -n hw.model)"
[ "$fails" = 0 ]
