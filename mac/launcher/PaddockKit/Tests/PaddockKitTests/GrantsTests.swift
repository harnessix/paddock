import XCTest
@testable import PaddockKit

/// Live grant edits of a running app, and paddock's refusals.
@MainActor
final class GrantsTests: XCTestCase {
    func makeModel() async throws -> (Harness, AppDetailModel) {
        let h = try await Harness("ready")
        let model = h.store.detail(for: "Sample Notes")
        await model.load()
        XCTAssertEqual(model.shares.count, 2)
        return (h, model)
    }

    func testShareAddIsLiveAtOnce() async throws {
        let (h, model) = try await makeModel()
        let started = h.clock.now()
        let outcome = await model.addFolder("/Users/demo/Pictures/Shots")
        guard case .added(let share) = outcome else { return XCTFail("\(outcome)") }
        // The row is there without waiting for an event: well within 2 s.
        XCTAssertEqual(h.clock.now().timeIntervalSince(started), 0)
        XCTAssertTrue(model.shares.contains { $0.path == "/Users/demo/Pictures/Shots" })
        XCTAssertEqual(model.label(for: share, now: h.clock.now()), .live)
        XCTAssertEqual(GrantLabel.live.text.english, "Live")
        XCTAssertEqual(h.calls(["app", "share", "Sample Notes", "add"]).last?.args,
                       ["app", "share", "Sample Notes", "add", "/Users/demo/Pictures/Shots", "--json"])
        XCTAssertTrue(model.toast?.text.english.hasSuffix("Visible in the app under /Volumes/My Shared Files/Shots.") ?? false)
    }

    func testModeChangeWithOpenFilesShowsReopenFor30Seconds() async throws {
        let (h, model) = try await makeModel()
        let projects = model.shares.first { $0.path == "/Users/demo/Projects" }!
        XCTAssertEqual(projects.mode, "rw")
        await model.setMode(projects, .ro)
        XCTAssertEqual(h.calls(["app", "share", "Sample Notes", "mode"]).last?.args,
                       ["app", "share", "Sample Notes", "mode", "/Users/demo/Projects", "ro", "--json"])
        let row = model.shares.first { $0.path == "/Users/demo/Projects" }!
        XCTAssertEqual(row.mode, "ro")
        XCTAssertEqual(model.label(for: row, now: h.clock.now()), .reopenFiles)
        XCTAssertEqual(GrantLabel.reopenFiles.text.english, "Reopen files")
        h.clock.advance(29)
        XCTAssertEqual(model.label(for: row, now: h.clock.now()), .reopenFiles)
        h.clock.advance(1)
        await settle()
        XCTAssertEqual(model.label(for: row, now: h.clock.now()), .live)
        XCTAssertTrue(model.reopenUntil.isEmpty)
    }

    func testRemoveWithOpenFilesAsksWithTheCount() async throws {
        let (h, model) = try await makeModel()
        let projects = model.shares.first { $0.path == "/Users/demo/Projects" }!
        let first = await model.removeFolder(projects)
        XCTAssertEqual(first, .confirm(openFiles: 2))
        XCTAssertTrue(h.calls(["app", "share", "Sample Notes", "remove"]).isEmpty)
        XCTAssertEqual(AppDetailModel.removalText(app: "Sample Notes", openFiles: 2, path: "~/Projects").english,
                       "Sample Notes has 2 files open in ~/Projects. Removing the folder closes them for the app. Unsaved changes in those files can be lost.")
        let second = await model.removeFolder(projects, confirmed: true)
        XCTAssertEqual(second, .removed)
        XCTAssertFalse(model.shares.contains { $0.path == "/Users/demo/Projects" })
        // A folder without open files goes at once.
        let notes = model.shares.first { $0.path == "/Users/demo/Documents/Notes" }!
        let third = await model.removeFolder(notes)
        XCTAssertEqual(third, .removed)
    }

    func testDeviceToggleIsNextStartWithRestart() async throws {
        let (h, model) = try await makeModel()
        let camera = model.devices.first { $0.name == "Camera" }!
        XCTAssertFalse(model.deviceChangedWhileRunning)
        await model.setDevice(camera, on: true)
        XCTAssertEqual(h.calls(["app", "device"]).last?.args, ["app", "device", "Sample Notes", "Camera", "on"])
        XCTAssertTrue(model.deviceChangedWhileRunning)
        XCTAssertEqual(model.devices.first { $0.name == "Camera" }?.applies, "next_start")
        XCTAssertEqual(GrantLabel.nextStart.text.english, "On next start")
    }

    func testRuleAddShowsRowAndYourChange() async throws {
        let (h, model) = try await makeModel()
        let message = await model.addRule(host: "files.example.com", answer: .allow, scope: .always)
        XCTAssertNil(message)
        XCTAssertEqual(h.calls(["app", "rules", "Sample Notes", "add"]).last?.args,
                       ["app", "rules", "Sample Notes", "add", "files.example.com", "--allow", "--always", "--json"])
        XCTAssertTrue(model.rules.contains { $0.host == "files.example.com" })
        XCTAssertEqual(model.changes.first?.text, "Allow always files.example.com")
        XCTAssertEqual(model.changes.first.map { AppDetailModel.sourceText($0.source).english }, "You")
        // The same change arriving on the stream is not listed twice.
        h.events.send(#"{"v":1,"kind":"grant","app":"Sample Notes","grant":"rule","op":"add","detail":"Allow always files.example.com","source":"you","time":"2026-11-06T10:00:01Z"}"#)
        await settle()
        XCTAssertEqual(model.changes.filter { $0.text == "Allow always files.example.com" }.count, 1)
    }

    func testPanelAnswerShowsInChanges() async throws {
        let (h, model) = try await makeModel()
        h.events.send(#"{"v":1,"kind":"grant","app":"Sample Notes","grant":"rule","op":"add","detail":"Allow always cdn.static.example.com","source":"question_panel","time":"2026-11-06T10:00:02Z"}"#)
        await waitUntil { model.changes.first?.text == "Allow always cdn.static.example.com" }
        XCTAssertEqual(AppDetailModel.sourceText(model.changes.first!.source).english, "Question panel")
    }

    func testRuleValidation() async throws {
        XCTAssertEqual(AppDetailModel.checkHost("api.example.com"), .ok("api.example.com"))
        XCTAssertEqual(AppDetailModel.checkHost("*.example.com"), .ok("*.example.com"))
        XCTAssertEqual(AppDetailModel.checkHost("*"), .everyHost)
        XCTAssertEqual(AppDetailModel.checkHost("example"), .invalid)
        XCTAssertEqual(AppDetailModel.checkHost("bad host.com"), .invalid)
        let (h, model) = try await makeModel()
        let invalid = await model.addRule(host: "not a host", answer: .allow, scope: .always)
        XCTAssertEqual(invalid?.english, "Enter a host name, for example api.example.com.")
        XCTAssertTrue(h.calls(["app", "rules", "Sample Notes", "add", "not"]).isEmpty)
        let every = await model.addRule(host: "*", answer: .allow, scope: .always)
        XCTAssertEqual(every?.english, "A rule for every host needs the terminal: paddock app rules Sample Notes add '*' --yes-all-hosts")
    }

    func testSeededRulesStay() async throws {
        let (h, model) = try await makeModel()
        await model.removeRules(["sample.example.net"])
        XCTAssertTrue(model.rules.contains { $0.host == "sample.example.net" })
        XCTAssertTrue(h.calls(["app", "rules", "Sample Notes", "remove"]).isEmpty)
        await model.removeRules(["tracker.example.net"])
        XCTAssertFalse(model.rules.contains { $0.host == "tracker.example.net" })
    }

    func testClipboardAndNetworkAreLive() async throws {
        let (h, model) = try await makeModel()
        await model.setClipboard(toApp: false, fromApp: true)
        XCTAssertEqual(h.calls(["app", "clipboard"]).last?.args,
                       ["app", "clipboard", "Sample Notes", "--to-app", "off", "--from-app", "on"])
        await model.setNetwork(.none)
        XCTAssertEqual(h.calls(["app", "network"]).last?.args, ["app", "network", "Sample Notes", "none"])
        XCTAssertEqual(model.networkNote?.english, "Open connections close now.")
        XCTAssertEqual(model.networkMode, .none)
    }

    // MARK: Refusals

    func testLibraryIsRefusedVerbatimWithoutARow() async throws {
        let (h, model) = try await makeModel()
        let outcome = await model.addFolder("/Users/demo/Library")
        XCTAssertEqual(outcome, .refused(line: "~/Library is private to this Mac and cannot be shared."))
        XCTAssertFalse(model.shares.contains { $0.path == "/Users/demo/Library" })
        XCTAssertEqual(h.calls(["app", "share", "Sample Notes", "add", "/Users/demo/Library"]).count, 1)
    }

    func testEnvFolderOffersShareAnywayWithUnprotected() async throws {
        let (h, model) = try await makeModel()
        let first = await model.addFolder("/Users/demo/Code/site")
        XCTAssertEqual(first, .needsUnprotected(line: "~/Code/site has a .env file. The app could read the keys in it."))
        XCTAssertFalse(model.shares.contains { $0.path == "/Users/demo/Code/site" })
        let second = await model.addFolder("/Users/demo/Code/site", unprotected: true)
        guard case .added = second else { return XCTFail("\(second)") }
        XCTAssertEqual(h.calls(["app", "share", "Sample Notes", "add", "/Users/demo/Code/site"]).last?.args,
                       ["app", "share", "Sample Notes", "add", "/Users/demo/Code/site", "--unprotected", "--json"])
    }
}
