import XCTest

/// The accessibility audit on every screen, labels on every status symbol,
/// and keyboard traversal.
final class AccessibilityUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = true
    }

    /// Runs the audit and fails once per issue with the element, so a
    /// failure line names what to fix. No issue is ignored.
    private func audit(_ app: XCUIApplication, _ screen: String,
                       file: StaticString = #filePath, line: UInt = #line) {
        do {
            try app.performAccessibilityAudit { issue in
                let element = issue.element.map { element in
                    "\(element.elementType.rawValue) id=\(element.identifier) label=\(element.label) "
                        + "value=\(String(describing: element.value)) frame=\(element.frame)"
                } ?? "no element"
                XCTFail("\(screen): \(issue.compactDescription): \(element)", file: file, line: line)
                return true
            }
        } catch {
            XCTFail("\(screen): \(error)", file: file, line: line)
        }
    }

    func testOnboardingSteps() {
        let fake = Fake("fresh")
        let app = fake.launch()
        XCTAssertTrue(app.buttons["onboarding-continue"].waitForExistence(timeout: 10))
        audit(app, "onboarding step 1")
        app.buttons["onboarding-continue"].click()
        XCTAssertTrue(app.element("prereq-chip").waitForExistence(timeout: 10))
        audit(app, "onboarding step 2")
        let next = app.buttons["onboarding-continue"]
        expectation(for: NSPredicate(format: "isEnabled == true"), evaluatedWith: next)
        waitForExpectations(timeout: 10)
        next.click()
        audit(app, "onboarding step 3")
        app.buttons["onboarding-build"].click()
        waitForText(app, "of 13.1 GB")
        audit(app, "onboarding step 4")
    }

    func testMainWindowScreensAndSheets() {
        let empty = Fake("empty").launch()
        XCTAssertTrue(empty.element("empty-state").waitForExistence(timeout: 10))
        audit(empty, "empty state")
        empty.terminate()

        let app = Fake("ready", env: ["PADDOCK_LAUNCHER_PICK_APP": "/Applications/Notebook.app"]).launch()
        app.selectApp("Sample Notes")
        audit(app, "overview")
        app.openTab("Grants")
        audit(app, "grants")
        app.openTab("Log")
        audit(app, "log")
        app.element("sidebar-questions").click()
        audit(app, "questions")
        app.element("sidebar-base-image").click()
        audit(app, "base image")

        app.buttons["add-app-button"].click()
        XCTAssertTrue(app.buttons["add-confirm"].waitForExistence(timeout: 10))
        audit(app, "add app sheet")
        app.typeKey(.escape, modifierFlags: [])

        app.selectApp("Sample Mail")
        // The detail keeps the tab of the last app: Log, from above.
        app.openTab("Overview")
        app.buttons["remove-app-button"].click()
        audit(app, "remove sheet")
        app.typeKey(.escape, modifierFlags: [])

        app.selectApp("Sample Notes")
        app.openTab("Grants")
        app.buttons["add-rule-button"].click()
        audit(app, "add rule sheet")
        app.typeKey(.escape, modifierFlags: [])

        app.menuBars.menuBarItems.element(boundBy: 1).click()
        app.menuItems["Uninstall…"].click()
        audit(app, "uninstall sheet")
        app.typeKey(.escape, modifierFlags: [])
    }

    func testSettingsPanes() {
        let app = Fake("ready").launch()
        XCTAssertTrue(app.element("sidebar").waitForExistence(timeout: 10))
        app.typeKey(",", modifierFlags: .command)
        for pane in ["General", "Storage", "Network", "Advanced"] {
            let row = app.settingsRow(pane)
            XCTAssertTrue(row.waitForExistence(timeout: 5), pane)
            row.click()
            audit(app, "settings " + pane)
        }
    }

    // Every status symbol carries a state word, not only a color.
    func testEveryStatusSymbolHasALabel() {
        let app = Fake("ready").launch()
        XCTAssertTrue(app.element("sidebar").waitForExistence(timeout: 10))
        for name in ["Sample Notes", "Sample Mail"] {
            let symbol = app.element("status-symbol-" + name)
            XCTAssertTrue(symbol.exists, name)
            XCTAssertFalse(symbol.label.isEmpty, name)
        }
        XCTAssertEqual(app.element("status-symbol-Sample Notes").label, "Running")
        XCTAssertEqual(app.element("status-symbol-Sample Mail").label, "Ready")
    }

    // Tab moves the keyboard focus through the controls and every stop has one
    // focused element. AppleKeyboardUIMode is the system setting "Keyboard
    // navigation". A launch argument does not change it. 2 means every
    // control; anything else means Tab reaches only lists and text fields.
    func testKeyboardTraversalReachesControls() throws {
        let raw = CFPreferencesCopyAppValue("AppleKeyboardUIMode" as CFString, kCFPreferencesAnyApplication)
        let mode = (raw as? NSNumber)?.intValue
        try XCTSkipIf(mode != 2, "Full keyboard access is off (AppleKeyboardUIMode is \(mode.map(String.init) ?? "unset")). Tab reaches only lists and text fields until Keyboard navigation is on in System Settings.")
        let app = Fake("ready", arguments: ["-AppleKeyboardUIMode", "2"]).launch()
        app.selectApp("Sample Notes")
        let focused = app.descendants(matching: .any).matching(NSPredicate(format: "hasKeyboardFocus == true"))
        var seen = Set<String>()
        for _ in 0..<40 {
            app.typeKey(.tab, modifierFlags: [])
            let element = focused.firstMatch
            if element.exists { seen.insert(element.identifier + "|" + element.label) }
        }
        XCTAssertGreaterThan(seen.count, 5, "Tab reached \(seen.count) controls: \(seen)")
        XCTAssertTrue(seen.contains { $0.hasPrefix("start-button") || $0.hasPrefix("open-button") || $0.contains("Stop") },
                      "Tab did not reach the Start/Open/Stop buttons: \(seen)")
    }
}
