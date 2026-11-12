#if DEBUG
import AppKit
import PaddockKit
import QuartzCore

/// Debug builds only: PADDOCK_LAUNCHER_SNAPSHOT=DIR draws every window to
/// DIR/N-TITLE.png after a few seconds and quits. No screen permission:
/// each window draws its own layer tree into a bitmap.
///
/// - PADDOCK_LAUNCHER_SNAPSHOT_DELAY=SECONDS: the wait before the dump (5).
/// - PADDOCK_LAUNCHER_SNAPSHOT_SELECT=NAME: selects the app with this name,
///   instance key or title, or `Questions`, in the main window first.
@MainActor
enum SnapshotDump {
    static func scheduleIfAsked() {
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["PADDOCK_LAUNCHER_SNAPSHOT"] else { return }
        let delay = Double(env["PADDOCK_LAUNCHER_SNAPSHOT_DELAY"] ?? "5") ?? 5
        let select = env["PADDOCK_LAUNCHER_SNAPSHOT_SELECT"].flatMap { $0.isEmpty ? nil : $0 }
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
            Task { @MainActor in
                if let select {
                    self.select(select)
                    // The detail view loads after the selection changes.
                    await wait(seconds: 1)
                }
                await dump(to: URL(fileURLWithPath: dir))
                NSApp.terminate(nil)
            }
        }
    }

    /// Selects the sidebar item NAME, as a click on it would.
    private static func select(_ name: String) {
        let apps = AppContainer.store.apps.map { Row(key: $0.key, name: $0.name, title: $0.title) }
        guard let item = sidebarItem(for: name, apps: apps) else {
            let names = (["Questions"] + apps.map(\.key)).joined(separator: ", ")
            FileHandle.standardError.write(Data("snapshot: no sidebar item \"\(name)\"; there are: \(names)\n".utf8))
            return
        }
        AppContainer.showMain(item)
    }

    struct Row: Equatable {
        var key: String
        var name: String
        var title: String
    }

    /// The sidebar item of NAME: `Questions` in any case, else an app by
    /// instance key, then name or title, then the same without case.
    static func sidebarItem(for name: String, apps: [Row]) -> SidebarItem? {
        let wanted = name.trimmingCharacters(in: .whitespaces)
        let lower = wanted.lowercased()
        if lower == "questions" { return .questions }
        let app = apps.first { $0.key == wanted }
            ?? apps.first { $0.name == wanted || $0.title == wanted }
            ?? apps.first { $0.key.lowercased() == lower || $0.name.lowercased() == lower || $0.title.lowercased() == lower }
        return app.map { .app($0.key) }
    }

    private static func dump(to dir: URL) async {
        NSApp.activate()
        let windows = NSApp.windows.filter(\.isVisible)
        for (n, window) in windows.enumerated() {
            // A window that is not key draws its controls inactive: gray
            // prominent buttons, a dim sidebar selection.
            if window.canBecomeKey { window.makeKeyAndOrderFront(nil) } else { window.orderFront(nil) }
            await wait(seconds: 0.3)
            window.displayIfNeeded()
            CATransaction.flush()
            await nextPass()
            guard let data = png(of: window) else {
                FileHandle.standardError.write(Data("snapshot: could not draw \"\(window.title)\"\n".utf8))
                continue
            }
            let name = window.title.isEmpty ? "window" : window.title.replacingOccurrences(of: "/", with: "-")
            try? data.write(to: dir.appendingPathComponent("\(n)-\(name).png"))
        }
    }

    /// The window frame view (title bar included) drawn through its layer
    /// tree. `cacheDisplay` misses layer-hosted content such as the
    /// sidebar `List`; `render(in:)` draws what Core Animation holds.
    private static func png(of window: NSWindow) -> Data? {
        guard let view = window.contentView?.superview ?? window.contentView else { return nil }
        guard let layer = view.layer else { return cachedPNG(of: view) }
        let scale = window.backingScaleFactor
        let size = view.bounds.size
        let width = Int((size.width * scale).rounded())
        let height = Int((size.height * scale).rounded())
        guard width > 0, height > 0,
              let space = CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                                      space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return cachedPNG(of: view) }
        // Materials (the sidebar) blur what is behind the window, which a
        // layer render cannot see: start from the window background.
        var background = CGColor(gray: 1, alpha: 1)
        window.effectiveAppearance.performAsCurrentDrawingAppearance {
            background = NSColor.windowBackgroundColor.cgColor
        }
        context.setFillColor(background)
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.scaleBy(x: scale, y: scale)
        // render(in:) applies the flips of the tree itself, as on screen.
        layer.render(in: context)
        guard let image = context.makeImage() else { return cachedPNG(of: view) }
        let rep = NSBitmapImageRep(cgImage: image)
        rep.size = size
        return rep.representation(using: .png, properties: [:])
    }

    /// The old path, for a view without a layer.
    private static func cachedPNG(of view: NSView) -> Data? {
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return nil }
        view.cacheDisplay(in: view.bounds, to: rep)
        return rep.representation(using: .png, properties: [:])
    }

    /// One pass of the main run loop: layout, display and the Core
    /// Animation commit run before the next main-queue block.
    private static func nextPass() async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            RunLoop.main.perform(inModes: [.common]) { continuation.resume() }
        }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.main.async { continuation.resume() }
        }
    }

    private static func wait(seconds: Double) async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { continuation.resume() }
        }
    }
}
#endif
