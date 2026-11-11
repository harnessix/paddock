import AppKit
import PaddockKit
import XCTest
@testable import PaddockLauncher

/// The status item icon, the error table strings through the catalog, and
/// the release checks of the signed archive.
@MainActor
final class AppChecks: XCTestCase {
    // The status item uses template images: monochrome, tinted by the system.
    func testMenuBarIconIsATemplate() {
        for state in [MenuModel.IconState.idle, .running, .question] {
            let image = MenuBarIcon.image(for: state)
            XCTAssertTrue(image.isTemplate, "\(state)")
            XCTAssertGreaterThan(image.size.width, 0, "\(state): symbol \(MenuBarIcon.symbol(for: state)) missing")
        }
    }

    // The live status item, when the menu bar extra is on screen.
    func testStatusItemButtonImageIsTemplate() throws {
        let buttons = NSApp.windows.compactMap { $0.contentView }.flatMap { AppChecks.buttons(in: $0) }
            .filter { String(describing: type(of: $0)).contains("StatusBar") }
        try XCTSkipIf(buttons.isEmpty, "no status item window in this test host")
        for button in buttons {
            XCTAssertEqual(button.image?.isTemplate, true)
        }
    }

    static func buttons(in view: NSView) -> [NSButton] {
        var found: [NSButton] = []
        if let button = view as? NSButton { found.append(button) }
        for sub in view.subviews { found += buttons(in: sub) }
        return found
    }

    // Each row of the error table resolves through the catalog to the exact
    // title and primary button.
    func testErrorTableThroughTheCatalog() {
        let rows: [(LauncherError, String?, String?)] = [
            (.notInstalled, "paddock is not installed", "Check Again"),
            (.tooOld(have: "0.2.1", need: "0.2.2"), "Update paddock", "Copy Command"),
            (.tooNew(have: "1.0.0"), "Update paddock for Mac", "Copy Command"),
            (.twoRunning(app: "Sample Mail", running: []), "Two apps are already running", "Stop…"),
            (.lowDiskAtStart(app: "Sample Notes", needBytes: 1, freeBytes: 0), "Not enough free space to start Sample Notes", "Manage Storage…"),
            (.appChanged(app: "Sample Notes"), nil, nil),
            (.appCopyFailed(app: "Sample Notes", line: "x"), "Sample Notes runs from the previous copy", "Show Log"),
            (.crashed(app: "Sample Notes", crashRow: nil), "Sample Notes stopped unexpectedly", "Start Again"),
            (.agentAway, nil, nil),
            (.goldenRemoved, nil, "Build…"),
            (.streamLost, "Live updates paused", "Refresh"),
        ]
        for (error, title, primary) in rows {
            XCTAssertEqual(error.title?.localized, title)
            XCTAssertEqual(error.primary.title?.localized, primary)
            XCTAssertFalse(error.body?.localized.isEmpty ?? true)
        }
    }

    func testBundleIdentifier() {
        XCTAssertEqual(Bundle.main.bundleIdentifier, "dev.paddock.mac")
    }
}

/// The release checks of the signed, notarized build. Scripts/check-build.sh
/// archives the app with warnings as errors and sets these variables.
final class ReleaseChecks: XCTestCase {
    private var env: [String: String] { ProcessInfo.processInfo.environment }

    @discardableResult
    private func run(_ tool: String, _ args: [String]) throws -> (Int32, String) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = args
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        try process.run()
        process.waitUntilExit()
        return (process.terminationStatus, String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self))
    }

    func testArchivedAppIsSignedAndAccepted() throws {
        let app = try XCTUnwrap(env["PADDOCK_LAUNCHER_ARCHIVED_APP"].flatMap { $0.isEmpty ? nil : $0 },
                                "run Scripts/check-build.sh, which sets PADDOCK_LAUNCHER_ARCHIVED_APP")
        let (verify, verifyOut) = try run("/usr/bin/codesign", ["--verify", "--strict", "--deep", "--verbose=2", app])
        XCTAssertEqual(verify, 0, verifyOut)
        let (assess, assessOut) = try run("/usr/sbin/spctl", ["--assess", "--type", "execute", "--verbose", app])
        XCTAssertEqual(assess, 0, assessOut)
    }

    func testDMGIsStapled() throws {
        let dmg = try XCTUnwrap(env["PADDOCK_LAUNCHER_DMG"].flatMap { $0.isEmpty ? nil : $0 },
                                "run Scripts/check-build.sh with a notary profile, which sets PADDOCK_LAUNCHER_DMG")
        let (status, out) = try run("/usr/bin/xcrun", ["stapler", "validate", dmg])
        XCTAssertEqual(status, 0, out)
    }

    override func setUpWithError() throws {
        try XCTSkipUnless(env["PADDOCK_LAUNCHER_RELEASE_CHECKS"] == "1",
                          "release checks run from Scripts/check-build.sh")
    }
}
