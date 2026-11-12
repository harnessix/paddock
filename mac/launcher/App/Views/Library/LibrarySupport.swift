import AppKit
import Observation
import PaddockKit
import SwiftUI

/// The apps that wait for the review sheet. A drop on the window, a drop
/// on the Dock icon and `Add App…` all go through here, so every path shows
/// the same sheet, one app at a time.
@MainActor
@Observable
final class AddCenter {
    static let shared = AddCenter()

    var queue = AddQueue()
    /// The line for a drop that held no app.
    var refusal: UIText?

    /// Queues the apps of a drop and opens the main window on the first.
    func receive(_ paths: [String]) {
        let intake = DropIntake.sort(paths)
        refusal = intake.apps.isEmpty && !intake.rejected.isEmpty ? LibraryCopy.notAnApp : nil
        if let refusal { AppContainer.store.show(refusal) }
        guard !intake.apps.isEmpty else { return }
        queue.enqueue(intake.apps)
        AppContainer.showMain(nil)
        presentNext()
    }

    /// Opens the sheet for the next app when no sheet shows.
    func presentNext() {
        guard AppContainer.router.sheet == nil, let path = queue.current else { return }
        AppContainer.router.sheet = .addApp(path: path)
    }

    /// The sheet of `path` closed.
    func finished(_ path: String?) {
        if let path, queue.current == path { queue.advance() }
        // The next sheet opens after this one has gone.
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 300_000_000)
            self.presentNext()
        }
    }

    func cancelAll() {
        queue.clear()
    }
}

/// The first appearance of the main window in this process: the launch
/// questions and the test drop run once, not at each reopen.
@MainActor
enum LaunchOnce {
    private static var done = false

    static func claim() -> Bool {
        guard !done else { return false }
        done = true
        return true
    }
}

/// Brings the windows of a running session forward: the session is the
/// Dock app `<Name> (paddock)` of paddock's shim.
@MainActor
enum SessionActivator {
    static func dockName(_ app: AppItem) -> String {
        app.title + " (paddock)"
    }

    /// True when a session app was found and asked to come forward.
    @discardableResult
    static func activate(_ app: AppItem) -> Bool {
        let name = dockName(app)
        guard let running = NSWorkspace.shared.runningApplications.first(where: { $0.localizedName == name }) else {
            return false
        }
        return running.activate()
    }
}

/// The launcher's own place on disk and the move into ~/Applications.
@MainActor
enum SelfInstaller {
    static let declinedKey = "selfInstallDeclined"

    static var bundlePath: String { Bundle.main.bundlePath }

    static var place: SelfInstall.Place {
        let url = URL(fileURLWithPath: bundlePath)
        let readOnly = (try? url.resourceValues(forKeys: [.volumeIsReadOnlyKey]))?.volumeIsReadOnly ?? false
        return SelfInstall.place(bundlePath: bundlePath, home: NSHomeDirectory(), readOnlyVolume: readOnly)
    }

    static var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
    }

    /// Asks once at launch when the launcher runs outside an Applications
    /// folder. Never in a UI test.
    static func offerIfNeeded() {
        guard !AppContainer.isUITest else { return }
        let declined = LauncherDefaults.store.bool(forKey: declinedKey)
        guard SelfInstall.shouldOffer(place, declined: declined) else { return }
        let alert = NSAlert()
        alert.messageText = SelfInstall.question.localized
        alert.informativeText = SelfInstall.questionBody.localized
        alert.addButton(withTitle: String(localized: "Move to Applications Folder"))
        alert.addButton(withTitle: String(localized: "Do Not Move"))
        if alert.runModal() == .alertFirstButtonReturn {
            move()
        } else {
            LauncherDefaults.store.set(true, forKey: declinedKey)
        }
    }

    /// Copies the bundle into ~/Applications, opens the copy and quits. A
    /// failure shows the reason; the launcher keeps running where it is.
    static func move() {
        let files = FileManager.default
        let target = SelfInstall.target(home: NSHomeDirectory())
        let targetURL = URL(fileURLWithPath: target)
        let existing = files.fileExists(atPath: target)
            ? (Bundle(path: target)?.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0") : nil
        do {
            switch SelfInstall.decision(existing: existing, mine: version) {
            case .openExisting:
                break
            case .replace:
                try files.trashItem(at: targetURL, resultingItemURL: nil)
                try copy(to: targetURL)
            case .copy:
                try files.createDirectory(at: targetURL.deletingLastPathComponent(), withIntermediateDirectories: true)
                try copy(to: targetURL)
            }
        } catch {
            fail(error)
            return
        }
        let original = URL(fileURLWithPath: bundlePath)
        let removeOriginal = SelfInstall.removesOriginal(place)
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.createsNewApplicationInstance = true
        NSWorkspace.shared.openApplication(at: targetURL, configuration: configuration) { _, error in
            Task { @MainActor in
                if let error {
                    fail(error)
                    return
                }
                if removeOriginal { try? FileManager.default.trashItem(at: original, resultingItemURL: nil) }
                NSApp.terminate(nil)
            }
        }
    }

    private static func copy(to target: URL) throws {
        try FileManager.default.copyItem(at: URL(fileURLWithPath: bundlePath), to: target)
    }

    private static func fail(_ error: Error) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = String(localized: "paddock for Mac was not moved")
        alert.informativeText = error.localizedDescription
        alert.runModal()
    }
}

/// A full-window state of the library: loading, paddock offline, or a
/// failed first load. Each names the next step.
@MainActor
struct LibraryStateView: View {
    enum Kind {
        case loading
        case offline
        case failed(String)
    }

    let kind: Kind
    @Environment(LauncherStore.self) private var store

    var body: some View {
        switch kind {
        case .loading:
            ProgressView { Text("Loading apps…") }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .accessibilityIdentifier("library-loading")
        case .offline:
            PlainEmpty {
                Label { Text("paddock is not available") } icon: { DecorativeSymbol(name: "bolt.horizontal.circle") }
            } description: {
                Text(LibraryCopy.offline)
            } actions: {
                Button("Check Again") { Task { await AppContainer.checkAgain() } }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("library-check-again")
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text("paddock is not available"))
            .accessibilityIdentifier("library-offline")
        case .failed(let line):
            PlainEmpty {
                Label { Text("The apps did not load") } icon: { DecorativeSymbol(name: "exclamationmark.triangle") }
            } description: {
                Text(verbatim: line).labelInk()
            } actions: {
                Button("Try Again") { Task { await store.refresh() } }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .keyboardShortcut(.defaultAction)
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text("The apps did not load"))
            .accessibilityIdentifier("library-error")
        }
    }
}

/// The highlight while an app hovers over the window: a dashed stroke in
/// the accent color and one line. Nothing else moves.
@MainActor
struct DropHighlight: View {
    let active: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            if active {
                RoundedRectangle(cornerRadius: 10)
                    .strokeBorder(Color.accentColor, style: StrokeStyle(lineWidth: 3, dash: [10, 6]))
                    .background(Color.accentColor.opacity(0.06), in: RoundedRectangle(cornerRadius: 10))
                    .overlay(alignment: .bottom) {
                        Label { Text("Drop to add") } icon: { DecorativeSymbol(name: "plus.app") }
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            .background(.regularMaterial, in: Capsule())
                            .padding(16)
                    }
                    .padding(6)
                    .transition(.opacity)
                    .accessibilityIdentifier("drop-highlight")
            }
        }
        .allowsHitTesting(false)
        .animation(reduceMotion ? nil : .default, value: active)
    }
}
