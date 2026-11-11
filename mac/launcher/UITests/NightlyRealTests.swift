import XCTest

/// Checks against the real paddock on a Mac. They run only
/// with PADDOCK_LAUNCHER_REAL=1 and need a built base image and one app
/// profile, named in PADDOCK_LAUNCHER_REAL_APP. Sessions run with
/// `--network none`. Run them with Scripts/nightly-real.sh.
final class NightlyRealTests: XCTestCase {
    private var env: [String: String] { ProcessInfo.processInfo.environment }
    private var paddock: String { env["PADDOCK_LAUNCHER_REAL_BIN"] ?? "/opt/homebrew/bin/paddock" }
    private var appName: String { env["PADDOCK_LAUNCHER_REAL_APP"] ?? "" }

    override func setUpWithError() throws {
        try XCTSkipUnless(env["PADDOCK_LAUNCHER_REAL"] == "1", "set PADDOCK_LAUNCHER_REAL=1 to run against the real paddock")
        try XCTSkipIf(appName.isEmpty, "set PADDOCK_LAUNCHER_REAL_APP to an app profile name")
        continueAfterFailure = false
    }

    @discardableResult
    private func run(_ args: [String]) throws -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: paddock)
        process.arguments = args
        let pipe = Pipe()
        process.standardOutput = pipe
        try process.run()
        process.waitUntilExit()
        return String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    }

    private func json(_ args: [String]) throws -> [String: Any] {
        let text = try run(args)
        return (try JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any]) ?? [:]
    }

    private func waitForState(_ state: String, timeout: TimeInterval = 120) throws -> [String: Any] {
        let end = Date().addingTimeInterval(timeout)
        while Date() < end {
            let status = try json(["app", "status", appName, "--json"])
            if let app = status["app"] as? [String: Any], app["state"] as? String == state { return status }
            Thread.sleep(forTimeInterval: 2)
        }
        XCTFail("\(appName) did not reach \(state)")
        return [:]
    }

    /// The real panel's keys: Esc denies, Return allows once, ⌘A always
    /// allows. The panel belongs to paddock's shim; its bundle id comes from
    /// PADDOCK_LAUNCHER_REAL_SHIM. The app must ask on start (network ask).
    func testPanelKeys() throws {
        let shimID = try XCTUnwrap(env["PADDOCK_LAUNCHER_REAL_SHIM"], "set PADDOCK_LAUNCHER_REAL_SHIM")
        try run(["app", "network", appName, "ask"])
        try run(["app", "run", appName])
        _ = try waitForState("running")
        let shim = XCUIApplication(bundleIdentifier: shimID)
        let keys: [(XCUIKeyboardKey, XCUIElement.KeyModifierFlags, String)] = [
            (.escape, [], "deny"), (.return, [], "allow-once"), (XCUIKeyboardKey(rawValue: "a"), .command, "allow-always"),
        ]
        for (key, modifiers, expected) in keys {
            XCTAssertTrue(shim.windows.firstMatch.waitForExistence(timeout: 60), "no question panel")
            let before = try json(["approvals", "--json"])["questions"] as? [[String: Any]] ?? []
            let id = try XCTUnwrap(before.first?["id"] as? String)
            shim.windows.firstMatch.typeKey(key, modifierFlags: modifiers)
            Thread.sleep(forTimeInterval: 2)
            let log = try run(["app", "log", appName, "--events", "--tail", "50"])
            XCTAssertTrue(log.contains(id) && log.contains(expected), "\(expected) not sent for \(id)")
        }
        try run(["app", "stop", appName])
    }

    /// Start, add a read-only share live, see it in the guest, remove it,
    /// stop, start warm.
    func testLiveShareAndWarmStart() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("paddock-nightly-share")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try run(["app", "network", appName, "none"])
        try run(["app", "run", appName])
        _ = try waitForState("running")

        let added = try json(["app", "share", appName, "add", folder.path, "--json"])
        XCTAssertEqual(added["applies"] as? String, "live")
        let status = try json(["app", "status", appName, "--json"])
        let shares = status["shares"] as? [[String: Any]] ?? []
        let share = try XCTUnwrap(shares.first { $0["path"] as? String == folder.path })
        XCTAssertTrue((share["guest_path"] as? String ?? "").hasPrefix("/Volumes/My Shared Files"))
        XCTAssertEqual(share["mode"] as? String, "ro")

        try run(["app", "share", appName, "remove", folder.path, "--json"])
        try run(["app", "stop", appName])
        _ = try waitForState("saved")

        let start = try run(["app", "run", appName])
        XCTAssertTrue(start.lowercased().contains("warm"), "the start line has no warm: \(start)")
        try run(["app", "stop", appName])
    }
}
