import XCTest

/// The main window, the menu bar extra, questions, quit, remove and uninstall.
final class MainWindowUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    // MARK: App states

    func testAppRowWalksThroughEveryState() {
        let fake = Fake("walk")
        let app = fake.launch()
        let steps: [(String, String)] = [
            ("Stopped", "Stopped"),
            ("Starting… cloning", "Starting"),
            ("Starting… booting", "Starting"),
            ("Starting… copying app", "Starting"),
            ("Starting… opening", "Starting"),
            ("Running · 3 windows", "Running"),
            ("Stopping…", "Stopping"),
            ("Ready · saved", "Ready"),
            ("The VM stopped: out of memory", "Error: The VM stopped: out of memory"),
            ("Build the base image to start this app.", "Blocked: Build the base image to start this app."),
        ]
        let status = app.element("status-text-Sample Notes")
        XCTAssertTrue(status.waitForExistence(timeout: 10))
        for (text, spoken) in steps {
            let shows = NSPredicate(format: "label BEGINSWITH %@ OR value BEGINSWITH %@", text, text)
            expectation(for: shows, evaluatedWith: status)
            waitForExpectations(timeout: 8)
            let row = app.element("app-row-Sample Notes")
            XCTAssertTrue(row.label.contains(spoken), "\(row.label) has no \(spoken)")
        }
    }

    // MARK: Empty state and add app

    func testEmptyStateOffersAddApp() {
        let app = Fake("empty").launch()
        XCTAssertTrue(app.element("empty-state").waitForExistence(timeout: 10))
        waitForText(app, "No apps yet")
        XCTAssertTrue(app.buttons["empty-add-app"].exists)
    }

    func testAddAppAppendsRowAndShowsToast() {
        let fake = Fake("added", env: ["PADDOCK_LAUNCHER_PICK_APP": "/Applications/Notebook.app"])
        let app = fake.launch()
        app.buttons["add-app-button"].click()
        let add = app.buttons["add-confirm"]
        XCTAssertTrue(add.waitForExistence(timeout: 10))
        expectation(for: NSPredicate(format: "isEnabled == true"), evaluatedWith: add)
        waitForExpectations(timeout: 10)
        add.click()
        XCTAssertTrue(app.element("app-row-Notebook").waitForExistence(timeout: 10))
        waitForText(app, "It also appears in Launchpad as \"Notebook (paddock)\".")
        XCTAssertFalse(fake.calls("app install Notebook").isEmpty)
    }

    func testAddAppRefusals() {
        let cases: [(String, String, String?)] = [
            ("precheck-app-store", "This app came from the App Store and needs an Apple ID", "Open website"),
            ("precheck-no-golden", "Build the base image first.", "add-build"),
            ("precheck-low-space", "Needs 2.7 GB, 1.1 GB free.", "Manage Storage…"),
        ]
        for (scenario, message, button) in cases {
            let app = Fake(scenario, env: ["PADDOCK_LAUNCHER_PICK_APP": "/Applications/Notebook.app"]).launch()
            app.buttons["add-app-button"].click()
            XCTAssertTrue(app.element("add-message").waitForExistence(timeout: 10), scenario)
            waitForText(app, message)
            XCTAssertFalse(app.buttons["add-confirm"].isEnabled, scenario)
            if let button { XCTAssertTrue(app.buttons[button].exists, scenario) }
            app.terminate()
        }
    }

    // MARK: Questions

    func testQuestionsBadgeAnswerAndPause() {
        let fake = Fake("questions")
        let app = fake.launch()
        app.element("sidebar-questions").click()
        // A question open event adds a row.
        XCTAssertTrue(app.element("question-q3").waitForExistence(timeout: 10))
        app.openStatusMenu()
        XCTAssertTrue(app.menuItems["Open questions… (3)"].waitForExistence(timeout: 5))
        app.typeKey(.escape, modifierFlags: [])
        // Answering from the list sends allow-once for that id.
        app.buttons["answer-allow-once-q1"].click()
        XCTAssertTrue(fake.waitForCall("approvals answer q1 --allow-once"))
        // The panel's answer removes the row.
        let gone = NSPredicate(format: "exists == false")
        expectation(for: gone, evaluatedWith: app.element("question-q3"))
        waitForExpectations(timeout: 10)
        // Asking paused: the banner.
        XCTAssertTrue(app.element("paused-banner").waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["deny-all"].exists)
        XCTAssertTrue(app.buttons["ask-one-by-one"].exists)
    }

    // The launcher never shows its own panel for a question.
    func testNoSecondPanelForAQuestion() {
        let app = Fake("questions").launch()
        app.element("sidebar-questions").click()
        XCTAssertTrue(app.element("question-q3").waitForExistence(timeout: 10))
        XCTAssertEqual(app.windows.count, 1, "only the main window")
        XCTAssertEqual(app.dialogs.count, 0)
        XCTAssertEqual(app.sheets.count, 0)
        XCTAssertTrue(app.windows.firstMatch.identifier.hasPrefix("main"), "the one window is the main window")
    }

    // MARK: Quit

    func testQuitKeepsAppsRunning() {
        let fake = Fake("ready")
        let app = fake.launch()
        app.openStatusMenu()
        XCTAssertTrue(app.menuItems["1 app keeps running"].waitForExistence(timeout: 5))
        // The app menu has the same title, from the bundle name "paddock for Mac".
        // The click belongs to the status menu this test just opened.
        let quit = app.statusItems.firstMatch.menuItems["Quit paddock for Mac"]
        XCTAssertTrue(quit.waitForExistence(timeout: 5))
        quit.click()
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10))
        XCTAssertTrue(fake.calls("app stop").isEmpty)
    }

    func testQuitStopsAppsWhenTheSettingIsOff() {
        let fake = Fake("ready", env: ["PADDOCK_LAUNCHER_KEEP_RUNNING": "0"])
        let app = fake.launch()
        XCTAssertTrue(app.element("app-row-Sample Notes").waitForExistence(timeout: 10))
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(fake.waitForCall("app stop Sample Notes"))
        XCTAssertEqual(app.state, .runningForeground, "waits for saved")
        fake.inject(#"{"v":1,"kind":"session","app":"Sample Notes","state":"saved","since":"2026-11-06T10:00:00Z"}"#)
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 15))
    }

    // MARK: Remove app

    func testRemoveAppSheet() {
        let fake = Fake("ready")
        let app = fake.launch()
        app.selectApp("Sample Mail")
        app.buttons["remove-app-button"].click()
        let body = app.element("remove-body")
        XCTAssertTrue(body.waitForExistence(timeout: 5))
        let text = body.text
        XCTAssertTrue(text.contains("(3.2 GB with its saved state 2.0 GB)") && text.contains("(3.4 GB)"), text)
        XCTAssertTrue(text.contains("paddock trash for 7 days"), text)
        app.buttons["remove-confirm"].click()
        waitForText(app, "Sample Mail removed. 6.6 GB freed.")
        XCTAssertEqual(fake.calls("app remove"), ["app remove Sample Mail --yes --json"])
    }

    func testRemoveWithDeleteData() {
        let fake = Fake("stopped")
        let app = fake.launch()
        app.selectApp("Sample Notes")
        app.buttons["remove-app-button"].click()
        let deleteData = app.checkBoxes["remove-delete-data"]
        XCTAssertTrue(deleteData.waitForExistence(timeout: 5))
        deleteData.click()
        let on = NSPredicate(format: "value == 1")
        expectation(for: on, evaluatedWith: deleteData)
        waitForExpectations(timeout: 5)
        app.buttons["remove-confirm"].click()
        XCTAssertTrue(fake.waitForCall("app remove Sample Notes --delete-data --yes --json"), "\(fake.calls.map(\.line))")
    }

    func testRemoveRunningAppIsBlocked() {
        let app = Fake("ready").launch()
        app.selectApp("Sample Notes")
        app.buttons["remove-app-button"].click()
        XCTAssertTrue(app.buttons["remove-confirm"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["remove-confirm"].isEnabled)
        waitForText(app, "Stop Sample Notes first.")
        XCTAssertTrue(app.sheets.buttons["Stop"].exists)
    }

    // MARK: Uninstall

    func testUninstallOrderAndNoBrew() {
        let fake = Fake("stopped")
        let app = fake.launch()
        XCTAssertTrue(app.element("sidebar").waitForExistence(timeout: 10))
        app.menuBars.menuBarItems.element(boundBy: 1).click()
        app.menuItems["Uninstall…"].click()
        waitForText(app, "Remove the base image (25.3 GB)")
        waitForText(app, "Remove sandboxed apps and their saved states (6.4 GB)")
        waitForText(app, "Remove app data (6.8 GB)")
        app.buttons["uninstall-remove"].click()
        XCTAssertTrue(app.element("uninstall-done").waitForExistence(timeout: 15))
        XCTAssertEqual(app.text("uninstall-done"),
                       "Done. 38.5 GB freed. You can now drag paddock for Mac to the Trash.")
        let removals = fake.calls.map(\.line).filter { $0.contains("remove") }
        XCTAssertEqual(removals, ["app remove Sample Notes --yes --json", "app remove Sample Mail --yes --json", "macvm remove --yes --json"])
        XCTAssertFalse(fake.calls.contains { $0.line.contains("brew") })
    }

    // MARK: Menu bar extra

    func testMenuBarListsEightAppsThenMore() {
        let app = Fake("many-apps").launch()
        app.openStatusMenu()
        XCTAssertTrue(app.menuItems["Base image ready"].waitForExistence(timeout: 5))
        for i in 1...8 { XCTAssertTrue(app.menuItems["App \(i)"].exists, "App \(i)") }
        XCTAssertFalse(app.menuItems["App 9"].exists)
        XCTAssertTrue(app.menuItems["More…"].exists)
        XCTAssertFalse(app.menuItems["1 app keeps running"].exists)
    }

    // MARK: Stream loss and polling

    func testStreamLossShowsPausedAndRecovers() {
        let fake = Fake("ready")
        let app = fake.launch()
        XCTAssertTrue(app.element("sidebar").waitForExistence(timeout: 10))
        XCTAssertFalse(app.element("stream-status").exists)
        fake.killEvents(true)
        XCTAssertTrue(app.element("stream-status").waitForExistence(timeout: 10))
        waitForText(app, "Live updates paused ·")
        // A link-style button shows as a link on macOS.
        let line = app.element("stream-status")
        let refresh = line.buttons["Refresh"].exists ? line.buttons["Refresh"] : line.links["Refresh"]
        XCTAssertTrue(refresh.exists)
        // The stream comes back: Refresh recovers at once.
        fake.killEvents(false)
        refresh.click()
        let gone = NSPredicate(format: "exists == false")
        expectation(for: gone, evaluatedWith: app.element("stream-status"))
        waitForExpectations(timeout: 10)
        // And again without Refresh: the launcher reopens the stream itself.
        fake.killEvents(true)
        XCTAssertTrue(app.element("stream-status").waitForExistence(timeout: 10))
        fake.killEvents(false)
        expectation(for: gone, evaluatedWith: app.element("stream-status"))
        waitForExpectations(timeout: 15)
    }

    func testNoPollingWithAHealthyStream() {
        let fake = Fake("ready")
        let app = fake.launch()
        app.selectApp("Sample Notes")
        app.openTab("Grants")
        app.openTab("Overview")
        let start = Int(Date().timeIntervalSince1970)
        RunLoop.current.run(until: Date().addingTimeInterval(20))
        let status = fake.calls.filter { $0.line.contains(" status") && $0.time >= start }
        // One visible screen, 20 s: at most one status call per 5 s.
        XCTAssertLessThanOrEqual(status.count, 4, "\(status)")
        let times = status.map(\.time)
        for pair in zip(times, times.dropFirst()) {
            XCTAssertGreaterThanOrEqual(pair.1 - pair.0, 5)
        }
    }
}
