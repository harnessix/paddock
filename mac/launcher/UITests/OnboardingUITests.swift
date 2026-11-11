import XCTest

/// Binary discovery, the version gate, the checklist and the base image build.
final class OnboardingUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    /// Goes from step 1 to step 2 of the onboarding window.
    private func toCheckStep(_ app: XCUIApplication) {
        let next = app.buttons["onboarding-continue"]
        XCTAssertTrue(next.waitForExistence(timeout: 10))
        next.click()
    }

    // No paddock anywhere: the install screen with the exact command and Copy.
    func testMissingPaddockShowsInstallCommand() {
        let fake = Fake("fresh", binary: "/nonexistent/paddock")
        let app = fake.launch()
        toCheckStep(app)
        let row = app.element("prereq-paddock")
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        XCTAssertEqual(app.textFields["command-field"].value as? String, "brew install harnessix/tap/paddock")
        XCTAssertTrue(app.buttons["copy-button"].exists)
        XCTAssertTrue(app.buttons["check-again"].exists)
        XCTAssertFalse(app.buttons["onboarding-continue"].isEnabled)
    }

    func testOldPaddockShowsUpdatePaddock() {
        let app = Fake("old-version").launch()
        waitForText(app, "This version of paddock for Mac needs paddock 0.2.2 or newer. You have 0.2.1.")
        XCTAssertTrue(app.staticTexts["Update paddock"].exists)
        XCTAssertTrue(noText(app, "Update paddock for Mac"))
        XCTAssertTrue(app.buttons["Copy Command"].exists)
    }

    func testNewerMajorShowsUpdatePaddockForMac() {
        let app = Fake("new-version").launch()
        waitForText(app, "Update paddock for Mac")
        XCTAssertTrue(app.buttons["Copy Command"].exists)
    }

    func testEqualVersionPasses() {
        let fake = Fake("ready")
        let app = fake.launch()
        XCTAssertTrue(app.element("sidebar").waitForExistence(timeout: 10))
        XCTAssertTrue(noText(app, "Update paddock"))
    }

    // MARK: Checklist

    func testIntelMacIsADeadEnd() {
        let app = Fake("intel").launch()
        toCheckStep(app)
        let chip = app.element("prereq-chip")
        XCTAssertTrue(chip.waitForExistence(timeout: 10))
        waitForText(app, "paddock app mode needs a Mac with Apple silicon.")
        XCTAssertTrue(chip.buttons["Quit"].exists)
        XCTAssertFalse(app.buttons["onboarding-continue"].isEnabled)
    }

    func testOldMacOSOffersSystemSettings() {
        let app = Fake("old-macos").launch()
        toCheckStep(app)
        XCTAssertTrue(app.buttons["Update macOS in System Settings"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["onboarding-continue"].isEnabled)
    }

    func testLowDiskOffersManageStorage() {
        let app = Fake("low-disk").launch()
        toCheckStep(app)
        XCTAssertTrue(app.buttons["Manage Storage…"].waitForExistence(timeout: 10))
        waitForText(app, "Needs 40.0 GB, 12.0 GB free.")
        XCTAssertFalse(app.buttons["onboarding-continue"].isEnabled)
    }

    func testLowMemoryIsOnlyAWarning() {
        let app = Fake("low-memory").launch()
        toCheckStep(app)
        waitForText(app, "apps get 4 GB each, which is the minimum.")
        let next = app.buttons["onboarding-continue"]
        let enabled = NSPredicate(format: "isEnabled == true")
        expectation(for: enabled, evaluatedWith: next)
        waitForExpectations(timeout: 10)
    }

    // MARK: Build

    func testBuildProgressReplay() {
        let fake = Fake("fresh")
        let app = fake.launch()
        toCheckStep(app)
        let next = app.buttons["onboarding-continue"]
        expectation(for: NSPredicate(format: "isEnabled == true"), evaluatedWith: next)
        waitForExpectations(timeout: 10)
        next.click()
        app.buttons["onboarding-build"].click()
        XCTAssertTrue(fake.waitForCall("macvm setup --json"))
        // Download: percent, then the estimate once 30 s of stream time passed.
        waitForText(app, "min left")
        XCTAssertTrue(app.element("close-note").exists)
        XCTAssertEqual(app.text("close-note"),
                       "You can close this window; the build continues. Quitting paddock for Mac pauses it.")
        // Pause and resume from the stream.
        waitForText(app, "Paused · 1.2 of 13.1 GB")
        XCTAssertTrue(app.buttons["resume-button"].exists)
        // Install is indeterminate with its typical time; download shows Done.
        waitForText(app, "Installing macOS, about 8 min", timeout: 15)
        XCTAssertTrue(app.element("step-download").images["Done"].exists)
        waitForText(app, "Waiting for the setup window…", timeout: 15)
        waitForText(app, "Follow the steps in paddock's setup window.")
        // Seal, then done.
        XCTAssertTrue(app.element("onboarding-done").waitForExistence(timeout: 20))
        XCTAssertTrue(app.buttons["onboarding-add-app"].exists)
    }

    func testBuildFailuresMapToTheirButton() {
        let cases = [("failed-no-space", "Manage Storage…"),
                     ("failed-download", "Try Again"),
                     ("failed-image", "Choose another restore image…")]
        for (scenario, button) in cases {
            let app = Fake(scenario).launch()
            let primary = app.buttons["failure-primary"]
            XCTAssertTrue(primary.waitForExistence(timeout: 10), scenario)
            XCTAssertEqual(primary.label, button, scenario)
            XCTAssertTrue(app.element("failure-reason").exists)
            app.terminate()
        }
    }

    // Quitting during a build pauses it; the next launch shows Paused.
    func testQuitDuringBuildPausesAndRelaunchShowsPaused() {
        let fake = Fake("fresh")
        let app = fake.launch()
        toCheckStep(app)
        let next = app.buttons["onboarding-continue"]
        expectation(for: NSPredicate(format: "isEnabled == true"), evaluatedWith: next)
        waitForExpectations(timeout: 10)
        next.click()
        app.buttons["onboarding-build"].click()
        waitForText(app, "of 13.1 GB")
        app.typeKey("q", modifierFlags: .command)
        XCTAssertTrue(fake.waitForCall("macvm setup --pause-download"))
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 15))

        let paused = Fake("paused").launch()
        waitForText(paused, "Paused · 6.2 of 13.1 GB")
        XCTAssertTrue(paused.buttons["resume-button"].exists)
    }
}
