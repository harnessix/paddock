// The frames helper of `paddock macvm bench frames`. It captures the
// host window with this CGWindowNumber through ScreenCaptureKit at 120
// Hz for SECONDS, and prints one `FRAME T_MS` line per frame whose
// pixels differ from the frame before (the time is the host's epoch milliseconds of the frame's arrival).
// `paddock macvm bench` turns the gaps into the measure: 17 ms is 60 fps,
// and no gap over 50 ms.
//
// Usage: swift frames.swift WINDOW_NUMBER SECONDS
// Needs Screen Recording for the terminal that runs it: the grant goes
// to the script, not to paddock, which keeps its "no permission" line.

import AppKit
import CoreMedia
import Foundation
import ScreenCaptureKit

func die(_ s: String) -> Never {
  FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
  exit(1)
}

let args = CommandLine.arguments
guard args.count == 3, let number = Int(args[1]), let seconds = Int(args[2])
else { die("usage: frames.swift WINDOW_NUMBER SECONDS") }

final class Sink: NSObject, SCStreamOutput {
  var last: UInt64 = 0
  var lines: [String] = []
  let lock = NSLock()

  func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
    guard type == .screen, let pixels = CMSampleBufferGetImageBuffer(buffer) else { return }
    let t = Int64(Date().timeIntervalSince1970 * 1000)
    CVPixelBufferLockBaseAddress(pixels, .readOnly)
    defer { CVPixelBufferUnlockBaseAddress(pixels, .readOnly) }
    guard let base = CVPixelBufferGetBaseAddress(pixels) else { return }
    let height = CVPixelBufferGetHeight(pixels)
    let stride = CVPixelBufferGetBytesPerRow(pixels)
    // A cheap hash over every 64th byte: the counter in the text area
    // changes pixels of several rows.
    var h: UInt64 = 1469598103934665603
    let bytes = base.assumingMemoryBound(to: UInt8.self)
    var i = 0
    let total = height * stride
    while i < total {
      h = (h ^ UInt64(bytes[i])) &* 1099511628211
      i += 64
    }
    lock.lock()
    if h != last {
      last = h
      lines.append("FRAME \(t)")
    }
    lock.unlock()
  }
}

let sink = Sink()
let sem = DispatchSemaphore(value: 0)
var failure: String? = nil
Task {
  do {
    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
    guard let window = content.windows.first(where: { $0.windowID == CGWindowID(number) })
    else { failure = "no window with number \(number)"; sem.signal(); return }
    let filter = SCContentFilter(desktopIndependentWindow: window)
    let config = SCStreamConfiguration()
    config.width = Int(window.frame.width) * 2
    config.height = Int(window.frame.height) * 2
    config.minimumFrameInterval = CMTime(value: 1, timescale: 120)
    config.queueDepth = 8
    config.showsCursor = false
    let stream = SCStream(filter: filter, configuration: config, delegate: nil)
    try stream.addStreamOutput(sink, type: .screen, sampleHandlerQueue: DispatchQueue(label: "frames"))
    try await stream.startCapture()
    try await Task.sleep(nanoseconds: UInt64(seconds) * 1_000_000_000)
    try await stream.stopCapture()
  } catch {
    failure = "\(error)"
  }
  sem.signal()
}
sem.wait()
if let f = failure { die(f) }
sink.lock.lock()
print(sink.lines.joined(separator: "\n"))
sink.lock.unlock()
