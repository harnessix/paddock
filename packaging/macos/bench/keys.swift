// The key helper of `paddock macvm bench input`. It posts COUNT key
// events into the host window with this CGWindowNumber, GAP ms apart,
// after a click into it, and prints one
// `KEY c T_MS` line per key with the host's epoch milliseconds right
// before the post. The characters cycle through the lower case letters
// so the typed text grows and `paddock macvm bench` pairs each key with
// the `ax.value` event of the guest by the text.
//
// Usage: swift keys.swift WINDOW_NUMBER COUNT GAP_MS
// Needs Accessibility for the terminal that runs it (the events are
// synthetic).

import AppKit
import Foundation

func die(_ s: String) -> Never {
  FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
  exit(1)
}

let args = CommandLine.arguments
guard args.count == 4, let number = Int(args[1]), let count = Int(args[2]), let gap = Int(args[3])
else { die("usage: keys.swift WINDOW_NUMBER COUNT GAP_MS") }

func nowMs() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }

func post(_ e: CGEvent?) { e?.post(tap: .cghidEventTap) }

// The window's frame, to click into its middle so it is key.
guard let list = CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(number)) as? [[String: Any]],
  let info = list.first, let bounds = info[kCGWindowBounds as String] as? [String: CGFloat]
else { die("no window with number \(number)") }
let mid = CGPoint(x: bounds["X"]! + bounds["Width"]! / 2, y: bounds["Y"]! + bounds["Height"]! / 2)
post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: mid, mouseButton: .left))
usleep(50_000)
post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: mid, mouseButton: .left))
post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: mid, mouseButton: .left))
usleep(500_000)

// The US key codes of a to z, in letter order.
let codes: [CGKeyCode] = [0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13, 7, 16, 6]
let letters = Array("abcdefghijklmnopqrstuvwxyz")
var out = ""
for i in 0..<count {
  let k = i % 26
  let down = CGEvent(keyboardEventSource: nil, virtualKey: codes[k], keyDown: true)
  let up = CGEvent(keyboardEventSource: nil, virtualKey: codes[k], keyDown: false)
  let t = nowMs()
  post(down)
  post(up)
  out += "KEY \(letters[k]) \(t)\n"
  usleep(useconds_t(gap * 1000))
}
print(out, terminator: "")
