import XCTest

/// The app library and sessions against paddock 0.2.x JSON: drops and the
/// review, instances, suspended sessions, remove with the trash, storage
/// and clean up, the pause answer of a build, and a failed list.
final class LibraryUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    // MARK: Add by drop

    // Two apps and a text file dropped at once: the file is refused, the
    // apps come one after another, each with paddock's review.
    func testDropQueuesTheReviewOfEachApp() {
        let drop = "/Applications/Notebook.app:/Applications/Sample Tasks.app:/Users/demo/notes.txt"
        let fake = Fake("drop", env: ["PADDOCK_LAUNCHER_DROP": drop])
        let app = fake.launch()
        XCTAssertTrue(app.element("add-position").waitForExistence(timeout: 10))
        XCTAssertEqual(app.text("add-position"), "App 1 of 2")
        XCTAssertTrue(app.element("check-bundle").exists)
        XCTAssertTrue(app.element("check-golden").exists)
        let add = app.buttons["add-confirm"]
        expectation(for: NSPredicate(format: "isEnabled == true"), evaluatedWith: add)
        waitForExpectations(timeout: 10)
        XCTAssertTrue(fake.calls("app add /Applications/Notebook.app --dry-run --json").count == 1)
        add.click()
        XCTAssertTrue(fake.waitForCall("app add /Applications/Notebook.app --name notebook --memory 6G --json"),
                      "\(fake.calls.map(\.line))")
        XCTAssertTrue(fake.waitForCall("app install notebook"))
        // The second app: the App Store refusal, Add stays off.
        let message = app.element("add-message")
        XCTAssertTrue(message.waitForExistence(timeout: 10))
        XCTAssertTrue(message.text.hasPrefix("Sample Tasks comes from the App Store"), message.text)
        XCTAssertEqual(app.text("add-position"), "App 2 of 2")
        XCTAssertFalse(app.buttons["add-confirm"].isEnabled)
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertTrue(app.element("app-row-notebook").waitForExistence(timeout: 10))
        XCTAssertTrue(fake.calls("app add /Applications/Sample Tasks.app --name").isEmpty)
        XCTAssertTrue(fake.calls.allSatisfy { !$0.line.contains("--network") }, "app add has no network flag")
    }

    func testDropRefusalsShowTheirAction() {
        let cases: [(String, String)] = [("drop-no-golden", "add-build"), ("drop-low-space", "Manage Storage…")]
        for (scenario, button) in cases {
            let app = Fake(scenario, env: ["PADDOCK_LAUNCHER_DROP": "/Applications/Notebook.app"]).launch()
            XCTAssertTrue(app.element("add-message").waitForExistence(timeout: 10), scenario)
            XCTAssertTrue(app.buttons[button].exists, scenario)
            XCTAssertFalse(app.buttons["add-confirm"].isEnabled, scenario)
            app.terminate()
        }
    }

    func testDropOfAnAddedAppPointsToIt() {
        let app = Fake("drop", env: ["PADDOCK_LAUNCHER_DROP": "/Applications/Sample Notes.app"]).launch()
        waitForText(app, "This app is in paddock already, as sample-notes.")
        XCTAssertTrue(app.buttons["Show App"].exists)
        XCTAssertFalse(app.buttons["add-confirm"].isEnabled)
    }

    // MARK: Instances and sessions

    func testInstancesAreGroupedAndANewOneStarts() {
        let fake = Fake("library")
        let app = fake.launch()
        XCTAssertTrue(app.element("app-group-sample-notes").waitForExistence(timeout: 10))
        XCTAssertTrue(app.element("app-row-sample-notes--2").exists)
        XCTAssertEqual(app.text("status-text-sample-mail"), "Suspended · resumes in about 4 s")
        app.selectApp("sample-notes")
        XCTAssertEqual(app.text("detail-title"), "Sample Notes")
        XCTAssertTrue(app.element("instance-row-sample-notes--2").waitForExistence(timeout: 5))
        app.element("detail-more").click()
        app.menuItems["Open New Instance"].click()
        XCTAssertTrue(fake.waitForCall("app run sample-notes --new"))
        XCTAssertTrue(app.element("app-row-sample-notes--3").waitForExistence(timeout: 10))
    }

    func testInstanceStartUsesItsNumber() {
        let fake = Fake("library")
        let app = fake.launch()
        app.selectApp("sample-notes--2")
        XCTAssertEqual(app.text("detail-title"), "Sample Notes 2")
        app.buttons["start-button"].click()
        XCTAssertTrue(fake.waitForCall("app run sample-notes --instance 2"))
        XCTAssertTrue(fake.calls("app run").allSatisfy { !$0.contains("--json") })
    }

    func testSuspendedSessionResumes() {
        let fake = Fake("library")
        let app = fake.launch()
        app.selectApp("sample-mail")
        XCTAssertTrue(app.buttons["resume-button"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["stop-button"].exists)
        XCTAssertFalse(app.buttons["start-button"].exists)
        app.buttons["resume-button"].click()
        XCTAssertTrue(fake.waitForCall("app run sample-mail"))
        // A session event moves the row on.
        fake.inject(#"{"v":1,"t":"2026-11-08T10:00:00Z","kind":"session","app":"sample-mail","home":"default","state":"running","text":"resumed"}"#)
        let running = NSPredicate(format: "label BEGINSWITH 'Running' OR value BEGINSWITH 'Running'")
        expectation(for: running, evaluatedWith: app.element("status-text-sample-mail"))
        waitForExpectations(timeout: 10)
    }

    func testSuspendOnlyWhenPaddockHasIt() {
        let plain = Fake("library").launch()
        plain.selectApp("sample-notes")
        XCTAssertTrue(plain.buttons["open-button"].waitForExistence(timeout: 5))
        XCTAssertFalse(plain.buttons["suspend-button"].exists)
        plain.terminate()

        let fake = Fake("library-suspend")
        let app = fake.launch()
        app.selectApp("sample-notes")
        XCTAssertTrue(app.buttons["suspend-button"].waitForExistence(timeout: 5))
        app.buttons["suspend-button"].click()
        XCTAssertTrue(fake.waitForCall("app suspend sample-notes --json"))
    }

    // MARK: Remove

    func testRemoveStopsFirstThenNamesTheTrash() {
        let fake = Fake("library")
        let app = fake.launch()
        app.selectApp("sample-mail")
        app.element("detail-more").click()
        app.menuItems["Remove App…"].click()
        let remove = app.buttons["remove-confirm"]
        XCTAssertTrue(remove.waitForExistence(timeout: 5))
        XCTAssertFalse(remove.isEnabled, "a suspended session must stop first")
        waitForText(app, "Stop Sample Mail first.")
        app.sheets.buttons["Stop"].click()
        XCTAssertTrue(fake.waitForCall("app stop sample-mail"))
        fake.inject(#"{"v":1,"t":"2026-11-08T10:00:00Z","kind":"session","app":"sample-mail","home":"default","state":"stopped","text":"quit with save"}"#)
        expectation(for: NSPredicate(format: "isEnabled == true"), evaluatedWith: remove)
        waitForExpectations(timeout: 10)
        remove.click()
        XCTAssertTrue(fake.waitForCall("app remove sample-mail --yes --json"))
        waitForText(app, "Sample Mail removed. 3.2 GB freed. Its data (2.1 GB) stays in the paddock trash for 7 days.")
        XCTAssertFalse(app.element("app-row-sample-mail").exists)
    }

    // MARK: Storage

    func testStorageAndCleanUpPreview() {
        let fake = Fake("library")
        let app = fake.launch()
        XCTAssertTrue(app.element("sidebar").waitForExistence(timeout: 10))
        app.typeKey(",", modifierFlags: .command)
        let pane = app.settingsRow("Storage")
        XCTAssertTrue(pane.waitForExistence(timeout: 5))
        pane.click()
        XCTAssertTrue(app.element("storage-usage").waitForExistence(timeout: 10))
        XCTAssertEqual(app.text("storage-usage"), "paddock uses 61.5 GB.")
        XCTAssertTrue(app.element("storage-row-instance:sample-mail").exists)
        XCTAssertTrue(app.element("storage-row-golden").exists)
        let cleanUp = app.buttons["storage-clean-up"]
        // A grouped Form builds only the rows on screen. Settings is not
        // always the first window, so scroll every open window.
        app.revealAnywhere(cleanUp)
        cleanUp.click()
        XCTAssertTrue(app.element("cleanup-list").waitForExistence(timeout: 10))
        XCTAssertTrue(fake.waitForCall("gc --json"))
        waitForText(app, "Removed now")
        waitForText(app, "its login is lost")
        XCTAssertEqual(app.buttons["cleanup-remove"].label, "Remove 16.1 GB")
        app.checkBoxes["cleanup-include-logins"].click()
        XCTAssertTrue(fake.waitForCall("gc --homes --include-app-logins --json"))
        let remove = app.buttons["cleanup-remove"]
        expectation(for: NSPredicate(format: "label == 'Remove 20.3 GB'"), evaluatedWith: remove)
        waitForExpectations(timeout: 10)
        remove.click()
        XCTAssertTrue(fake.waitForCall("gc --yes --homes --include-app-logins --json"))
        XCTAssertTrue(app.element("cleanup-done").waitForExistence(timeout: 10))
        XCTAssertEqual(app.text("cleanup-done"), "16.1 GB freed.")
        XCTAssertTrue(fake.calls("gc").allSatisfy { !$0.contains("--worktrees") && !$0.contains("--dedupe") })
    }

    func testStorageEmpty() {
        let app = Fake("storage-empty").launch()
        XCTAssertTrue(app.element("sidebar").waitForExistence(timeout: 10))
        app.typeKey(",", modifierFlags: .command)
        app.settingsRow("Storage").click()
        XCTAssertTrue(app.element("storage-empty").waitForExistence(timeout: 10))
    }

    // MARK: Base image

    func testPauseAfterTheDownloadSaysWhy() {
        let fake = Fake("building-v1")
        let app = fake.launch()
        XCTAssertTrue(app.element("sidebar-base-image").waitForExistence(timeout: 10))
        app.element("sidebar-base-image").click()
        let pause = app.buttons["pause-button"]
        XCTAssertTrue(pause.waitForExistence(timeout: 10))
        pause.click()
        XCTAssertTrue(fake.waitForCall("macvm setup --pause-download --json"))
        XCTAssertTrue(app.element("build-note").waitForExistence(timeout: 5))
        XCTAssertEqual(app.text("build-note"), "The download is done. The later steps cannot pause.")
    }

    func testNewerMacOffersTheUpdate() {
        let fake = Fake("host-newer-v1")
        let app = fake.launch()
        app.element("sidebar-base-image").click()
        let update = app.buttons["update-golden"]
        XCTAssertTrue(update.waitForExistence(timeout: 10))
        update.click()
        app.buttons["confirm-action"].click()
        XCTAssertTrue(fake.waitForCall("macvm update --yes"))
        XCTAssertTrue(fake.calls("macvm setup").isEmpty, "an update is macvm update, not setup")
    }

    // MARK: States

    func testFailedListShowsTryAgain() {
        let fake = Fake("list-fails")
        let app = fake.launch()
        XCTAssertTrue(app.element("library-error").waitForExistence(timeout: 10))
        let before = fake.calls("app list").count
        app.buttons["Try Again"].click()
        XCTAssertTrue(fake.waitForCall("app list"))
        let end = Date().addingTimeInterval(5)
        while fake.calls("app list").count <= before, Date() < end { RunLoop.current.run(until: Date().addingTimeInterval(0.2)) }
        XCTAssertGreaterThan(fake.calls("app list").count, before)
    }

    func testAboutShowsBothVersions() {
        let app = Fake("library").launch()
        XCTAssertTrue(app.element("sidebar").waitForExistence(timeout: 10))
        app.typeKey(",", modifierFlags: .command)
        let versions = app.element("about-versions")
        if !versions.waitForExistence(timeout: 5) {
            // A grouped Form builds only the rows on screen. The first
            // window is the main window, so scroll Settings too.
            app.revealAnywhere(versions)
        }
        XCTAssertTrue(versions.exists)
        XCTAssertTrue(app.text("about-versions").hasSuffix("· paddock 0.2.2"), app.text("about-versions"))
    }
}
