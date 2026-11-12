import XCTest

/// The polish layer on the screens: the detail header and its pill, the
/// meters, the empty states, the onboarding steps and the menu rows.
final class PolishUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    // P2, P4: the header speaks the state word, the primary button is the
    // default, and the tabs are a segmented control under the header.
    func testHeaderPillAndSegmentedTabs() {
        let app = Fake("ready").launch()
        app.selectApp("Sample Notes")
        let pill = app.element("detail-status")
        XCTAssertTrue(pill.waitForExistence(timeout: 10))
        XCTAssertEqual(pill.label, "Running")
        XCTAssertTrue(app.buttons["open-button"].exists)
        XCTAssertTrue(app.buttons["stop-button"].exists)
        let open = app.buttons["open-button"].frame
        let stop = app.buttons["stop-button"].frame
        XCTAssertGreaterThan(open.minX, stop.minX, "the primary button is last")
        XCTAssertTrue(app.segmentedControls.buttons["Overview"].exists)
        XCTAssertTrue(app.segmentedControls.buttons["Grants"].exists)
        XCTAssertTrue(app.segmentedControls.buttons["Log"].exists)
        XCTAssertLessThan(app.element("detail-title").frame.maxY, app.segmentedControls.buttons["Overview"].frame.minY)
        // The status dot on the sidebar icon speaks the same word.
        XCTAssertEqual(app.element("status-symbol-Sample Notes").label, "Running")
        XCTAssertTrue(app.element("app-row-Sample Notes").label.hasPrefix("Sample Notes, Running, 3 windows"),
                      app.element("app-row-Sample Notes").label)
    }

    // P5: memory as a gauge with `6 of 16 GB`, disk as one bar with a legend.
    func testOverviewMeters() {
        let app = Fake("ready").launch()
        app.selectApp("Sample Notes")
        let gauge = app.element("memory-gauge")
        XCTAssertTrue(gauge.waitForExistence(timeout: 10))
        XCTAssertEqual(gauge.value as? String, "6 of 16 GB")
        let overview = app.element("disk-bar")
        XCTAssertTrue(app.revealAnywhere(overview), "no disk bar")
        let value = overview.value as? String ?? ""
        XCTAssertTrue(value.contains("App copy 1.2 GB") && value.contains("Data 3.4 GB") && value.contains("Saved state 2.0 GB"),
                      value)
        XCTAssertTrue(app.element("disk-legend-appCopy").exists)
        XCTAssertTrue(app.element("disk-total").label.contains("6.6 GB"), app.element("disk-total").label)
    }

    // P6: one sentence and one prominent action on an empty screen.
    func testEmptyStates() {
        let empty = Fake("empty").launch()
        XCTAssertTrue(empty.element("empty-state").waitForExistence(timeout: 10))
        XCTAssertTrue(empty.buttons["empty-add-app"].isEnabled)
        empty.terminate()

        let app = Fake("ready").launch()
        app.element("sidebar-questions").click()
        XCTAssertTrue(app.element("questions-empty").waitForExistence(timeout: 10))
        waitForText(app, "No open questions")
    }

    // P9: the step list says done, current and not started.
    func testOnboardingStepStates() {
        let app = Fake("fresh").launch()
        let first = app.element("onboarding-step-1")
        XCTAssertTrue(first.waitForExistence(timeout: 10))
        XCTAssertTrue(first.label.contains("Current step"), first.label)
        XCTAssertTrue(app.element("onboarding-step-2").label.contains("Not started"))
        app.buttons["onboarding-continue"].click()
        XCTAssertTrue(app.element("prereq-chip").waitForExistence(timeout: 10))
        XCTAssertTrue(app.element("onboarding-step-1").label.contains("Done"), app.element("onboarding-step-1").label)
        XCTAssertTrue(app.element("onboarding-step-2").label.contains("Current step"))
        let next = app.buttons["onboarding-continue"]
        expectation(for: NSPredicate(format: "isEnabled == true"), evaluatedWith: next)
        waitForExpectations(timeout: 10)
        // Return is Continue: the default button.
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(app.buttons["onboarding-build"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.element("space-gauge").exists)
    }

    // P10: each app row of the menu opens with its state line.
    func testMenuRowStartsWithTheState() {
        let app = Fake("ready").launch()
        app.openStatusMenu()
        let notes = app.statusItems.firstMatch.menuItems["Sample Notes"]
        XCTAssertTrue(notes.waitForExistence(timeout: 5))
        notes.hover()
        XCTAssertTrue(app.menuItems["Running · 3 windows"].waitForExistence(timeout: 5))
        app.typeKey(.escape, modifierFlags: [])
    }
}
