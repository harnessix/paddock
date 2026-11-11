import PaddockKit
import XCTest
@testable import PaddockLauncher

/// The texts of the network and privacy screens: they resolve through the
/// catalog, and every key of those files has a Chinese value.
@MainActor
final class NetworkTextChecks: XCTestCase {
    static let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // AppTests
        .deletingLastPathComponent() // launcher

    /// The files of the network and privacy part.
    static let files = [
        "App/Views/Network", "App/Views/RulesTable.swift", "App/Views/QuestionsView.swift",
        "App/Views/GrantsView.swift", "PaddockKit/Sources/PaddockKit/Network",
    ]

    func swiftFiles() -> [URL] {
        NetworkTextChecks.files.flatMap { item -> [URL] in
            let url = NetworkTextChecks.root.appendingPathComponent(item)
            if url.pathExtension == "swift" { return [url] }
            let walker = FileManager.default.enumerator(at: url, includingPropertiesForKeys: nil)
            return (walker?.allObjects as? [URL] ?? []).filter { $0.pathExtension == "swift" }
        }
    }

    func testEveryNetworkKeyHasChinese() throws {
        let literal = #""((?:[^"\\]|\\.)*)""#
        let calls = ["Text", "Button", "Label", "Toggle", "Picker", "Section", "TextField", "Menu", "TableColumn"]
        let callRE = try NSRegularExpression(pattern: #"(?<![\w.])(?:"# + calls.map { $0 + #"\("# }.joined(separator: "|")
                                             + #")\s*"# + literal)
        let uiTextRE = try NSRegularExpression(pattern: #"UIText\(\s*(?:key:\s*)?"# + literal)
        var keys = Set<String>()
        for file in swiftFiles() {
            let text = try String(contentsOf: file, encoding: .utf8)
            for line in text.components(separatedBy: "\n") {
                let code = line.components(separatedBy: "//").first ?? ""
                let range = NSRange(code.startIndex..., in: code)
                for regex in [callRE, uiTextRE] {
                    for match in regex.matches(in: code, range: range) {
                        if let r = Range(match.range(at: 1), in: code) {
                            keys.insert(code[r].replacingOccurrences(of: "\\\"", with: "\""))
                        }
                    }
                }
            }
        }
        XCTAssertGreaterThan(keys.count, 100)
        let catalogURL = NetworkTextChecks.root.appendingPathComponent("App/Resources/Localizable.xcstrings")
        let catalog = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: catalogURL)) as? [String: Any])
        let strings = try XCTUnwrap(catalog["strings"] as? [String: [String: Any]])
        var missing: [String] = []
        for key in keys.sorted() {
            let zh = (strings[key]?["localizations"] as? [String: Any])?["zh-Hans"] as? [String: Any]
            let value = (zh?["stringUnit"] as? [String: Any])?["value"] as? String
            if value == nil || value?.isEmpty == true {
                missing.append(key)
            } else if value!.components(separatedBy: "%@").count != key.components(separatedBy: "%@").count {
                missing.append(key + " (placeholders differ)")
            }
        }
        XCTAssertTrue(missing.isEmpty, "no Chinese value: " + missing.joined(separator: "\n"))
    }

    func testStateTextsResolve() {
        XCTAssertEqual(NetSubject.rules.loading.localized, "Loading rules…")
        XCTAssertEqual(NetSubject.connections.failedTitle.localized, "The connections did not load")
        XCTAssertEqual(GrantsPart.allCases.map(\.title.localized), ["Network", "Connections", "Files & Clipboard"])
        let q = QuestionJSON(id: "q1", app: "Sample Notes", host: "api.example.com", port: 443)
        XCTAssertEqual(QuestionChoices.row(q)[2].title.localized, "Allow example.com")
        XCTAssertEqual(QuestionChoices.detail(q).localized, "port 443 · example.com and all its subdomains")
    }

    func testEditorTargetKeepsTheRow() {
        let rule = NetRuleJSON(id: "rule-1", host: "api.example.com", port: .number(8443), verdict: "deny")
        let row = RuleRow(rules: [rule], host: rule.host, isSite: false)
        let target = RuleEditorTarget.edit(row)
        XCTAssertEqual(target.row?.id, "rule-1")
        XCTAssertEqual(target.draft.port, .other)
        XCTAssertEqual(target.draft.otherPort, "8443")
        XCTAssertEqual(target.draft.verdict, .deny)
        XCTAssertNil(RuleEditorTarget.add(RuleDraft()).row)
    }
}
