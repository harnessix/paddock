import XCTest

/// Starts paddock for Mac against the fake paddock and one fixture scenario.
///
/// The fake (FakePaddock/fake-paddock) replays mac/launcher/Fixtures and
/// logs every call to calls.log in a scratch folder. The tests read that log
/// to check which commands the launcher sent.
final class Fake {
    static let launcherRoot = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // UITests
        .deletingLastPathComponent() // launcher

    let app = XCUIApplication()
    let state: URL

    init(_ scenario: String, binary: String? = nil, env: [String: String] = [:], arguments: [String] = []) {
        state = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("paddock-fake-" + UUID().uuidString)
        try? FileManager.default.createDirectory(at: state, withIntermediateDirectories: true)
        var environment = [
            "PADDOCK_BIN": binary ?? Fake.launcherRoot.appendingPathComponent("FakePaddock/fake-paddock").path,
            "PADDOCK_FAKE_SCENARIO": Fake.launcherRoot.appendingPathComponent("Fixtures/" + scenario).path,
            "PADDOCK_FAKE_STATE": state.path,
            // Only PADDOCK_BIN counts: a paddock installed on this Mac stays out.
            "PADDOCK_LAUNCHER_SEARCH_ROOT": "/nonexistent",
            "PADDOCK_LAUNCHER_DEFAULTS": "dev.paddock.mac.uitest." + UUID().uuidString,
            "PADDOCK_LAUNCHER_UITEST": "1",
        ]
        environment.merge(env) { $1 }
        app.launchEnvironment = environment
        // The checks read English texts: the app runs in English whatever
        // the language of the Mac, unless a test names a language itself.
        let english = arguments.contains("-AppleLanguages") ? [] : ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launchArguments = english + arguments
    }

    @discardableResult
    func launch() -> XCUIApplication {
        app.launch()
        return app
    }

    /// The calls paddock got, one argument line each.
    var calls: [(time: Int, line: String)] {
        let text = (try? String(contentsOf: state.appendingPathComponent("calls.log"), encoding: .utf8)) ?? ""
        return text.split(separator: "\n").compactMap { row in
            guard let space = row.firstIndex(of: " "), let time = Int(row[..<space]) else { return nil }
            return (time, String(row[row.index(after: space)...]))
        }
    }

    func calls(_ prefix: String) -> [String] {
        calls.map(\.line).filter { $0.hasPrefix(prefix) }
    }

    /// Appends one event line to the running event stream.
    func inject(_ json: String) {
        let url = state.appendingPathComponent("inject.jsonl")
        let line = Data((json + "\n").utf8)
        if let handle = try? FileHandle(forWritingTo: url) {
            handle.seekToEndOfFile()
            handle.write(line)
            try? handle.close()
        } else {
            try? line.write(to: url)
        }
    }

    /// Ends the event stream, as when paddock dies. `false` lets it return.
    func killEvents(_ on: Bool) {
        let url = state.appendingPathComponent("kill-events")
        if on {
            FileManager.default.createFile(atPath: url.path, contents: Data())
        } else {
            try? FileManager.default.removeItem(at: url)
        }
    }

    /// Waits until a call with the prefix shows up.
    func waitForCall(_ prefix: String, timeout: TimeInterval = 10) -> Bool {
        let end = Date().addingTimeInterval(timeout)
        while Date() < end {
            if !calls(prefix).isEmpty { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
        return false
    }
}

extension XCUIApplication {
    /// Any element with the identifier.
    func element(_ id: String) -> XCUIElement {
        descendants(matching: .any)[id]
    }

    /// The text of an element: its value, else its label.
    func text(_ id: String) -> String {
        element(id).text
    }

    /// Scrolls `container` until `element` exists: down first, then up.
    /// A grouped Form builds only the rows on screen, so a row further
    /// down is not in the tree before it scrolls into view.
    @discardableResult
    func reveal(_ element: XCUIElement, in container: XCUIElement, steps: Int = 12) -> Bool {
        for delta in [-120.0, 120.0] {
            for _ in 0..<steps {
                if element.exists { return true }
                container.scroll(byDeltaX: 0, deltaY: delta)
            }
        }
        return element.exists
    }

    /// The settings sidebar row. The row is a button with an identifier.
    /// An outline is not always there, and a title such as Network is also
    /// a label in the main window, so the identifier wins.
    func settingsRow(_ title: String) -> XCUIElement {
        let identified = element("settings-" + title.lowercased())
        if identified.waitForExistence(timeout: 5) { return identified }
        let outlined = outlines.staticTexts[title]
        if outlined.exists { return outlined }
        return staticTexts[title]
    }

    func selectApp(_ name: String) {
        let row = element("app-row-" + name)
        XCTAssertTrue(row.waitForExistence(timeout: 10), "no row for \(name)")
        row.click()
    }

    /// Scrolls every window until `element` exists. Settings is often not
    /// the first window, so scrolling `windows.firstMatch` never reaches it.
    @discardableResult
    func revealAnywhere(_ element: XCUIElement) -> Bool {
        if element.exists { return true }
        let root = self.element("settings-root")
        if root.exists, reveal(element, in: root) { return true }
        for index in 0..<windows.count {
            if reveal(element, in: windows.element(boundBy: index)) { return true }
        }
        return element.exists
    }

    /// A segmented picker is also a set of radio buttons on macOS. Clicking
    /// that radio proxy does not change the picker's selection. The segment
    /// button does, so it wins when both exist.
    func openTab(_ title: String) {
        let segment = segmentedControls.buttons[title]
        let radio = radioButtons[title]
        let tab = tabs[title]
        let button = buttons[title]
        let end = Date().addingTimeInterval(5)
        while Date() < end {
            if segment.exists || radio.exists || tab.exists || button.exists { break }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        }
        let chosen = segment.exists ? segment : (radio.exists ? radio : (tab.exists ? tab : button))
        XCTAssertTrue(chosen.exists, "no tab \(title)")
        chosen.click()
    }

    /// The menu of the status item.
    func openStatusMenu() {
        let item = statusItems.firstMatch
        XCTAssertTrue(item.waitForExistence(timeout: 10), "no status item")
        item.click()
    }
}

extension XCUIElement {
    /// A static text keeps its string in the value on macOS; the label is
    /// empty. Other elements carry it in the label.
    var text: String {
        if let value = value as? String, !value.isEmpty { return value }
        return label
    }
}

/// Waits for an element whose label or value contains `text`.
func waitForText(_ app: XCUIApplication, _ text: String, timeout: TimeInterval = 10,
                 file: StaticString = #filePath, line: UInt = #line) {
    let predicate = NSPredicate(format: "label CONTAINS %@ OR value CONTAINS %@", text, text)
    let element = app.descendants(matching: .any).matching(predicate).firstMatch
    XCTAssertTrue(element.waitForExistence(timeout: timeout), "no text: \(text)", file: file, line: line)
}

func noText(_ app: XCUIApplication, _ text: String) -> Bool {
    let predicate = NSPredicate(format: "label CONTAINS %@ OR value CONTAINS %@", text, text)
    return !app.descendants(matching: .any).matching(predicate).firstMatch.exists
}
