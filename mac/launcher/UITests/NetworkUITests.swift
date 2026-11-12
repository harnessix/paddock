import XCTest

/// The network and privacy flows: the rules table, the rule editor, the
/// recent connections, the open questions and the clipboard of a session.
final class NetworkUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    /// The Grants tab of Sample Notes, on the part `part`.
    private func grants(_ fake: Fake, part: String = "Network", app name: String = "Sample Notes") -> XCUIApplication {
        let app = fake.launch()
        app.selectApp(name)
        app.openTab("Grants")
        app.openTab(part)
        return app
    }

    private func table(_ app: XCUIApplication) -> XCUIElement {
        let table = app.element("rules-table")
        XCTAssertTrue(table.waitForExistence(timeout: 10), "no rules table")
        return table
    }

    private func choose(_ app: XCUIApplication, _ popUp: String, _ item: String) {
        let button = app.popUpButtons[popUp]
        XCTAssertTrue(button.waitForExistence(timeout: 5), "no \(popUp)")
        button.click()
        app.menuItems[item].click()
    }

    // MARK: Rules

    func testRulesTableShowsSitesProfileAndMaker() {
        let app = grants(Fake("ready"))
        let rules = table(app)
        XCTAssertTrue(rules.staticTexts["example.com and its subdomains"].waitForExistence(timeout: 5))
        XCTAssertTrue(rules.staticTexts["From the app profile"].exists)
        XCTAssertTrue(rules.staticTexts["Question panel"].exists)
        XCTAssertTrue(rules.staticTexts["Until the app stops"].exists)
        XCTAssertTrue(rules.staticTexts["Any port"].exists)
    }

    func testAddRuleWithTheWholeSite() {
        let fake = Fake("ready")
        let app = grants(fake)
        app.buttons["add-rule-button"].click()
        let host = app.textFields["rule-host"]
        XCTAssertTrue(host.waitForExistence(timeout: 5))
        host.click()
        host.typeText("www.example.org")
        let site = app.element("rule-site")
        XCTAssertTrue(site.waitForExistence(timeout: 5), "no site toggle")
        site.click()
        app.buttons["rule-add"].click()
        XCTAssertTrue(fake.waitForCall("app rules Sample Notes add example.org --port 443 --allow --site --json"))
        XCTAssertTrue(table(app).staticTexts["example.org and its subdomains"].waitForExistence(timeout: 5))
        // The change is in the Changes list, made by You.
        app.openTab("Files & Clipboard")
        let change = app.descendants(matching: .any).matching(identifier: "change-row")
            .matching(NSPredicate(format: "label CONTAINS 'example.org'")).firstMatch
        XCTAssertTrue(app.reveal(change, in: app.element("grants-form")), "no change row")
        XCTAssertTrue(change.label.contains("You"), change.label)
    }

    func testAddRuleValidatesInline() {
        let fake = Fake("ready")
        let app = grants(fake)
        app.buttons["add-rule-button"].click()
        let host = app.textFields["rule-host"]
        XCTAssertTrue(host.waitForExistence(timeout: 5))
        host.click()
        host.typeText("not a host")
        app.buttons["rule-add"].click()
        XCTAssertEqual(app.text("rule-message"), "Enter a host name, for example api.example.com.")
        XCTAssertTrue(fake.calls("app rules Sample Notes add").isEmpty)
    }

    // A rule for every host stays a Terminal act: the sheet shows the command.
    func testEveryHostShowsTheTerminalCommand() {
        let fake = Fake("ready")
        let app = grants(fake)
        app.buttons["add-rule-button"].click()
        let host = app.textFields["rule-host"]
        XCTAssertTrue(host.waitForExistence(timeout: 5))
        host.click()
        host.typeText("*")
        app.buttons["rule-add"].click()
        XCTAssertTrue(app.element("rule-message").waitForExistence(timeout: 5))
        XCTAssertEqual(app.textFields["command-field"].value as? String,
                       "paddock app rules Sample Notes add '*' --yes-all-hosts")
        XCTAssertTrue(fake.calls("app rules Sample Notes add").isEmpty)
    }

    func testRefusalShowsPaddocksLine() {
        let app = grants(Fake("ready"))
        app.buttons["add-rule-button"].click()
        let host = app.textFields["rule-host"]
        XCTAssertTrue(host.waitForExistence(timeout: 5))
        host.click()
        host.typeText("10.0.0.1")
        app.buttons["rule-add"].click()
        XCTAssertTrue(app.element("rule-message").waitForExistence(timeout: 5))
        XCTAssertEqual(app.text("rule-message"),
                       "paddock refuses a rule for 10.0.0.1: the guest may not reach this Mac's local network.")
        // The sheet stays open with the host, so the person can fix it.
        XCTAssertTrue(app.buttons["rule-add"].exists)
    }

    func testEditARule() {
        let fake = Fake("ready")
        let app = grants(fake)
        let row = table(app).staticTexts["tracker.example.net"]
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        row.click()
        app.buttons["edit-rule-button"].click()
        XCTAssertTrue(app.textFields["rule-host"].waitForExistence(timeout: 5))
        XCTAssertEqual(app.textFields["rule-host"].value as? String, "tracker.example.net")
        choose(app, "rule-scope", "Always")
        app.buttons["rule-add"].click()
        XCTAssertTrue(fake.waitForCall(
            "app rules Sample Notes change session-7c7c7c --host tracker.example.net --port 443 --deny --always --json"))
    }

    func testRemoveASiteAsksFirst() {
        let fake = Fake("ready")
        let app = grants(fake)
        let row = table(app).staticTexts["example.com and its subdomains"]
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        row.click()
        app.buttons["remove-rule-button"].click()
        // The dialog is an alert sheet on macOS; it may drop the identifier.
        var confirm = app.buttons["confirm-remove-rules"]
        if !confirm.waitForExistence(timeout: 3) { confirm = app.sheets.buttons["Remove"] }
        if !confirm.exists { confirm = app.dialogs.buttons["Remove"] }
        XCTAssertTrue(confirm.waitForExistence(timeout: 5))
        XCTAssertTrue(fake.calls("app rules Sample Notes remove").isEmpty)
        confirm.click()
        XCTAssertTrue(fake.waitForCall("app rules Sample Notes remove rule-0a0a0a rule-0a0a0a-subdomains --json"))
        let gone = NSPredicate(format: "exists == false")
        expectation(for: gone, evaluatedWith: table(app).staticTexts["example.com and its subdomains"])
        waitForExpectations(timeout: 5)
    }

    func testProfileRuleCannotBeRemoved() {
        let app = grants(Fake("ready"))
        let row = table(app).staticTexts["sample.example.net"]
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        row.click()
        XCTAssertFalse(app.buttons["remove-rule-button"].isEnabled)
        XCTAssertFalse(app.buttons["edit-rule-button"].isEnabled)
    }

    func testEmptyRulesAndConnections() {
        let app = grants(Fake("ready"), app: "Sample Mail")
        XCTAssertTrue(app.element("rules-empty").waitForExistence(timeout: 10))
        app.openTab("Connections")
        XCTAssertTrue(app.element("connections-empty").waitForExistence(timeout: 10))
    }

    // An older paddock without the JSON commands: the update command, not
    // a raw error.
    func testOldPaddockShowsTheUpdateCommand() {
        let app = grants(Fake("old-cli"))
        XCTAssertTrue(app.element("net-update").waitForExistence(timeout: 10))
        XCTAssertEqual(app.textFields["command-field"].value as? String, "brew upgrade paddock")
        app.openTab("Connections")
        XCTAssertTrue(app.element("net-update").waitForExistence(timeout: 10))
    }

    // MARK: Connections

    func testAllowADeniedHost() {
        let fake = Fake("ready")
        let app = grants(fake, part: "Connections")
        let allow = app.buttons["allow-host-telemetry.example.net"]
        XCTAssertTrue(allow.waitForExistence(timeout: 10))
        allow.click()
        XCTAssertTrue(fake.waitForCall("app rules Sample Notes add telemetry.example.net --port 443 --allow --json"))
        XCTAssertTrue(app.element("connection-allowed").waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["allow-host-telemetry.example.net"].exists)
    }

    func testAllowADeniedSite() {
        let fake = Fake("ready")
        let app = grants(fake, part: "Connections")
        let site = app.buttons["allow-site-telemetry.example.net"]
        XCTAssertTrue(site.waitForExistence(timeout: 10))
        XCTAssertEqual(site.label, "Allow example.net and all its subdomains")
        // An IP address has no site: only Allow.
        XCTAssertTrue(app.buttons["allow-host-203.0.113.7"].exists)
        XCTAssertFalse(app.buttons["allow-site-203.0.113.7"].exists)
        site.click()
        XCTAssertTrue(fake.waitForCall("app rules Sample Notes add example.net --port * --allow --site --json"))
    }

    func testDeniedFilter() {
        let app = grants(Fake("ready"), part: "Connections")
        XCTAssertTrue(app.element("connection-api.example.com").waitForExistence(timeout: 10))
        // The radio proxy of a segmented picker does not change the selection.
        let denied = app.segmentedControls.buttons["Denied"]
        XCTAssertTrue(denied.waitForExistence(timeout: 5))
        denied.click()
        let gone = NSPredicate(format: "exists == false")
        expectation(for: gone, evaluatedWith: app.element("connection-api.example.com"))
        waitForExpectations(timeout: 5)
        XCTAssertTrue(app.element("connection-telemetry.example.net").exists)
    }

    // MARK: Questions

    func testQuestionAnswersMirrorThePanel() {
        let fake = Fake("questions")
        let app = fake.launch()
        app.element("sidebar-questions").click()
        let site = app.buttons["answer-allow-site-q1"]
        XCTAssertTrue(site.waitForExistence(timeout: 10))
        XCTAssertEqual(site.label, "Allow example.com and all its subdomains")
        XCTAssertTrue(app.buttons["answer-deny-q1"].exists)
        XCTAssertTrue(app.buttons["answer-allow-once-q1"].exists)
        app.element("answer-more-q1").click()
        XCTAssertTrue(app.menuItems["Allow this session"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.menuItems["Always deny"].exists)
        XCTAssertTrue(app.menuItems["Always allow api.example.com only"].exists)
        app.menuItems["Allow everything for 10 minutes"].click()
        XCTAssertTrue(fake.waitForCall("approvals answer q1 --allow-everything"))
        let gone = NSPredicate(format: "exists == false")
        expectation(for: gone, evaluatedWith: app.element("question-q1"))
        waitForExpectations(timeout: 5)
        app.buttons["answer-allow-site-q2"].click()
        XCTAssertTrue(fake.waitForCall("approvals answer q2 --allow-site"))
    }

    // The newest card is first and has the panel's keys (D14): Return
    // allows once, Esc denies. The older cards have no keys.
    func testNewestQuestionFirstWithPanelKeys() {
        let fake = Fake("questions")
        let app = fake.launch()
        app.element("sidebar-questions").click()
        let once = app.buttons["answer-allow-once-q3"]
        XCTAssertTrue(once.waitForExistence(timeout: 10))
        expectation(for: NSPredicate(format: "isEnabled == true"), evaluatedWith: once)
        waitForExpectations(timeout: 5)
        let q3 = app.element("question-q3").frame
        let q1 = app.element("question-q1").frame
        XCTAssertLessThan(q3.minY, q1.minY, "the newest question is on top")
        XCTAssertEqual(app.element("question-host-q3").text, "files.files.example.com")
        app.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(fake.waitForCall("approvals answer q3 --allow-once"), "\(fake.calls.map(\.line))")
        let gone = NSPredicate(format: "exists == false")
        expectation(for: gone, evaluatedWith: app.element("question-q3"))
        waitForExpectations(timeout: 5)
        // q2 is the newest now.
        let deny = app.buttons["answer-deny-q2"]
        expectation(for: NSPredicate(format: "isEnabled == true"), evaluatedWith: deny)
        waitForExpectations(timeout: 5)
        app.typeKey(.escape, modifierFlags: [])
        XCTAssertTrue(fake.waitForCall("approvals answer q2 --deny"), "\(fake.calls.map(\.line))")
        XCTAssertTrue(fake.calls("approvals answer q1").isEmpty, "an older card took a key")
        XCTAssertTrue(fake.calls("approvals answer").allSatisfy { !$0.contains("always") && !$0.contains("site") },
                      "a key wrote a permanent rule")
    }

    // MARK: Clipboard

    func testSessionClipboardLeavesTheAppSetting() {
        let fake = Fake("ready")
        let app = grants(fake, part: "Files & Clipboard")
        let session = app.element("session-clipboard-from-app")
        XCTAssertTrue(app.reveal(session, in: app.element("grants-form")), "no session clipboard toggle")
        XCTAssertTrue(app.element("session-same").exists)
        session.click()
        XCTAssertTrue(fake.waitForCall("app clipboard Sample Notes --to-app on --from-app on --session --json"))
        XCTAssertTrue(fake.calls("app clipboard Sample Notes --to-app on --from-app on --json").isEmpty)
        let differs = NSPredicate(format: "exists == false")
        expectation(for: differs, evaluatedWith: app.element("session-same"))
        waitForExpectations(timeout: 5)
    }

    func testAppClipboardWhileStopped() {
        let fake = Fake("ready")
        let app = grants(fake, part: "Files & Clipboard", app: "Sample Mail")
        let toApp = app.element("clipboard-to-app")
        XCTAssertTrue(app.reveal(toApp, in: app.element("grants-form")), "no clipboard toggle")
        // A stopped app has no session part.
        XCTAssertFalse(app.element("session-clipboard-to-app").exists)
        waitForText(app, "Applies at the next start.")
        toApp.click()
        XCTAssertTrue(fake.waitForCall("app clipboard Sample Mail --to-app off --from-app on --json"))
    }
}
