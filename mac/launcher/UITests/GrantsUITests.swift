import XCTest

/// Live grant edits of a running app, and paddock's refusals.
final class GrantsUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func grants(_ fake: Fake) -> XCUIApplication {
        let app = fake.launch()
        app.selectApp("ChatGPT")
        app.openTab("Grants")
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
        XCTAssertTrue(text.text.hasPrefix("ChatGPT has 2 files open in"), text.text)
        XCTAssertTrue(fake.calls("app share ChatGPT remove").isEmpty)
        app.sheets.buttons["Remove"].click()
        XCTAssertTrue(fake.waitForCall("app share ChatGPT remove /Users/demo/Projects"))
    }

    func testDeviceToggleIsNextStartWithRestart() {
        let fake = Fake("ready")
        let app = grants(fake)
        // Devices is below the fold of the Grants form.
        let camera = app.element("device-Camera")
        XCTAssertTrue(app.reveal(camera, in: app.element("grants-form")), "no Camera toggle")
        camera.click()
        XCTAssertTrue(fake.waitForCall("app device ChatGPT Camera on"))
        XCTAssertTrue(app.buttons["restart-app"].waitForExistence(timeout: 5))
        waitForText(app, "On next start")
    }

    func testAddRuleShowsRowAndYourChange() {
        let fake = Fake("ready")
        let app = grants(fake)
        app.buttons["add-rule-button"].click()
        let host = app.textFields["rule-host"]
        XCTAssertTrue(host.waitForExistence(timeout: 5))
        host.click()
        host.typeText("files.example.com")
        app.buttons["rule-add"].click()
        XCTAssertTrue(fake.waitForCall("app rules ChatGPT add files.example.com --allow --always --json"))
        XCTAssertTrue(app.element("rules-table").staticTexts["files.example.com"].waitForExistence(timeout: 5))
        // A change row is one element: its label holds time, text and source.
        let change = app.descendants(matching: .any).matching(identifier: "change-row")
            .matching(NSPredicate(format: "label CONTAINS 'files.example.com'")).firstMatch
        XCTAssertTrue(app.reveal(change, in: app.element("grants-form")), "no change row")
        XCTAssertTrue(change.label.contains("You"), change.label)
    }

    func testAddRuleValidatesInline() {
        let app = grants(Fake("ready"))
        app.buttons["add-rule-button"].click()
        let host = app.textFields["rule-host"]
        XCTAssertTrue(host.waitForExistence(timeout: 5))
        host.click()
        host.typeText("not a host")
        app.buttons["rule-add"].click()
        XCTAssertEqual(app.text("rule-message"), "Enter a host name, for example api.example.com.")
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
        XCTAssertTrue(fake.waitForCall("app share ChatGPT add /Users/demo/Code/site --unprotected"))
        XCTAssertTrue(folderRow(app, "/Users/demo/Code/site").waitForExistence(timeout: 5))
    }
}
