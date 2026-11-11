import XCTest

/// Live grant edits of a running app, and paddock's refusals.
final class GrantsUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func grants(_ fake: Fake) -> XCUIApplication {
        let app = fake.launch()
        app.selectApp("Sample Notes")
        app.openTab("Grants")
        app.openTab("Files & Clipboard")
        XCTAssertTrue(app.buttons["add-folder-button"].waitForExistence(timeout: 10))
        return app
    }

    private func folderRow(_ app: XCUIApplication, _ path: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: "folder-row")
            .containing(NSPredicate(format: "label == %@ OR value == %@", path, path)).firstMatch
    }

    func testAddFolderIsLiveWithinTwoSeconds() {
        let fake = Fake("ready", env: ["PADDOCK_LAUNCHER_PICK_FOLDER": "/Users/demo/Pictures/Shots"])
        let app = grants(fake)
        app.buttons["add-folder-button"].click()
        let row = folderRow(app, "/Users/demo/Pictures/Shots")
        XCTAssertTrue(row.waitForExistence(timeout: 2))
        XCTAssertEqual(row.staticTexts["grant-label"].text, "Live")
        waitForText(app, "Visible in the app under /Volumes/My Shared Files/Shots.")
    }

    func testModeChangeWithOpenFilesShowsReopenFiles() {
        let app = grants(Fake("ready"))
        let row = folderRow(app, "/Users/demo/Projects")
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        row.popUpButtons.firstMatch.click()
        app.menuItems["Read only"].click()
        XCTAssertTrue(row.staticTexts["Reopen files"].waitForExistence(timeout: 3))
        // Back to Live after 30 s.
        let live = NSPredicate(format: "label == 'Live' OR value == 'Live'")
        expectation(for: live, evaluatedWith: row.staticTexts["grant-label"])
        waitForExpectations(timeout: 40)
    }

    func testRemoveFolderWithOpenFilesAsks() {
        let fake = Fake("ready")
        let app = grants(fake)
        let row = folderRow(app, "/Users/demo/Projects")
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        row.menuButtons.firstMatch.click()
        app.menuItems["Remove"].click()
        let text = app.element("remove-folder-text")
        XCTAssertTrue(text.waitForExistence(timeout: 5))
        XCTAssertTrue(text.text.hasPrefix("Sample Notes has 2 files open in"), text.text)
        XCTAssertTrue(fake.calls("app share Sample Notes remove").isEmpty)
        app.sheets.buttons["Remove"].click()
        XCTAssertTrue(fake.waitForCall("app share Sample Notes remove /Users/demo/Projects"))
    }

    func testDeviceToggleIsNextStartWithRestart() {
        let fake = Fake("ready")
        let app = grants(fake)
        // Devices is below the fold of the Grants form.
        let camera = app.element("device-Camera")
        XCTAssertTrue(app.reveal(camera, in: app.element("grants-form")), "no Camera toggle")
        camera.click()
        XCTAssertTrue(fake.waitForCall("app device Sample Notes Camera on"))
        XCTAssertTrue(app.buttons["restart-app"].waitForExistence(timeout: 5))
        waitForText(app, "On next start")
    }

    // The open files show on the row before Remove; a folder that only
    // this session shares says so.
    func testFolderCaptionsShowOpenFilesAndSessionOnly() {
        let app = grants(Fake("ready"))
        let projects = folderRow(app, "/Users/demo/Projects")
        XCTAssertTrue(projects.waitForExistence(timeout: 5))
        XCTAssertTrue(projects.staticTexts["folder-caption"].text.contains("2 files open"),
                      projects.staticTexts["folder-caption"].text)
        let notes = folderRow(app, "/Users/demo/Documents/Notes")
        XCTAssertTrue(notes.staticTexts["folder-caption"].text.contains("This session only"),
                      notes.staticTexts["folder-caption"].text)
    }

    func testKeepASessionFolder() {
        let fake = Fake("ready")
        let app = grants(fake)
        let notes = folderRow(app, "/Users/demo/Documents/Notes")
        XCTAssertTrue(notes.waitForExistence(timeout: 5))
        notes.menuButtons.firstMatch.click()
        app.menuItems["Keep for Later Starts"].click()
        XCTAssertTrue(fake.waitForCall("app share Sample Notes keep /Users/demo/Documents/Notes --json"))
    }

    // MARK: Refusals

    func testLibraryIsRefusedVerbatim() {
        let fake = Fake("ready", env: ["PADDOCK_LAUNCHER_PICK_FOLDER": "/Users/demo/Library"])
        let app = grants(fake)
        app.buttons["add-folder-button"].click()
        XCTAssertTrue(app.element("refusal-line").waitForExistence(timeout: 5))
        XCTAssertEqual(app.text("refusal-line"), "~/Library is private to this Mac and cannot be shared.")
        app.sheets.buttons["OK"].click()
        XCTAssertFalse(folderRow(app, "/Users/demo/Library").exists)
    }

    func testEnvFolderOffersShareAnyway() {
        let fake = Fake("ready", env: ["PADDOCK_LAUNCHER_PICK_FOLDER": "/Users/demo/Code/site"])
        let app = grants(fake)
        app.buttons["add-folder-button"].click()
        XCTAssertTrue(app.buttons["share-anyway"].waitForExistence(timeout: 5))
        XCTAssertEqual(app.text("refusal-line"), "~/Code/site has a .env file. The app could read the keys in it.")
        XCTAssertTrue(app.sheets.buttons["Cancel"].exists)
        app.buttons["share-anyway"].click()
        XCTAssertTrue(fake.waitForCall("app share Sample Notes add /Users/demo/Code/site --unprotected"))
        XCTAssertTrue(folderRow(app, "/Users/demo/Code/site").waitForExistence(timeout: 5))
    }
}
