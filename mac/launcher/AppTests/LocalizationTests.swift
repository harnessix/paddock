import XCTest

/// The string catalog matches the code: no missing key, no unused key, no
/// literal that bypasses the catalog. The same rules as Scripts/strings.py.
final class LocalizationTests: XCTestCase {
    static let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // AppTests
        .deletingLastPathComponent() // launcher

    static let literal = #""((?:[^"\\]|\\.)*)""#
    static let calls = ["Text", "Button", "Label", "Toggle", "Picker", "Section", "LabeledContent", "TextField",
                        "Menu", "TableColumn", "ContentUnavailableView", "Window", "CommandMenu", "Link"]

    func regex(_ pattern: String) throws -> NSRegularExpression {
        try NSRegularExpression(pattern: pattern)
    }

    func matches(_ regex: NSRegularExpression, in line: String) -> [String] {
        let range = NSRange(line.startIndex..., in: line)
        return regex.matches(in: line, range: range).compactMap { match in
            Range(match.range(at: 1), in: line).map { String(line[$0]) }
        }
    }

    func unescape(_ text: String) -> String {
        text.replacingOccurrences(of: "\\\"", with: "\"").replacingOccurrences(of: "\\\\", with: "\\")
            .replacingOccurrences(of: "\\n", with: "\n")
    }

    func swiftFiles(_ folder: String) -> [URL] {
        let base = LocalizationTests.root.appendingPathComponent(folder)
        let walker = FileManager.default.enumerator(at: base, includingPropertiesForKeys: nil)
        return (walker?.allObjects as? [URL] ?? []).filter { $0.pathExtension == "swift" }
    }

    func testCatalogMatchesTheCode() throws {
        let names = LocalizationTests.calls.map { NSRegularExpression.escapedPattern(for: $0) + #"\("# }
            + [#"String\(localized:"#]
        let callRE = try regex(#"(?<![\w.])(?:"# + names.joined(separator: "|") + #")\s*"# + LocalizationTests.literal)
        let uiTextRE = try regex(#"UIText\(\s*(?:key:\s*)?"# + LocalizationTests.literal)
        let anyRE = try regex(LocalizationTests.literal)
        var keys: [String: String] = [:]
        var problems: [String] = []
        for (folder, inApp) in [("App", true), ("PaddockKit/Sources", false)] {
            for file in swiftFiles(folder) {
                let text = try String(contentsOf: file, encoding: .utf8)
                for (number, line) in text.components(separatedBy: "\n").enumerated() {
                    let marked = line.trimmingCharacters(in: .whitespaces).hasSuffix("// localized")
                    let code = marked ? line : (line.components(separatedBy: "//").first ?? "")
                    var found = matches(uiTextRE, in: code)
                    if inApp {
                        found += matches(callRE, in: code)
                        if marked { found += matches(anyRE, in: line.components(separatedBy: "// localized")[0]) }
                        if code.contains("Text(verbatim: \"") {
                            problems.append("\(file.lastPathComponent):\(number + 1): Text(verbatim:) with a literal")
                        }
                    }
                    for raw in found {
                        if raw.contains("\\(") {
                            problems.append("\(file.lastPathComponent):\(number + 1): interpolation in a localized literal")
                            continue
                        }
                        keys[unescape(raw)] = keys[unescape(raw)] ?? "\(file.lastPathComponent):\(number + 1)"
                    }
                }
            }
        }
        let catalogURL = LocalizationTests.root.appendingPathComponent("App/Resources/Localizable.xcstrings")
        let catalog = try JSONSerialization.jsonObject(with: Data(contentsOf: catalogURL)) as? [String: Any]
        let have = Set((catalog?["strings"] as? [String: Any] ?? [:]).keys)
        for key in Set(keys.keys).subtracting(have).sorted() {
            problems.append("\(keys[key]!): missing in the catalog: \(key)")
        }
        for key in have.subtracting(keys.keys).sorted() {
            problems.append("unused key in the catalog: \(key)")
        }
        XCTAssertTrue(problems.isEmpty, problems.joined(separator: "\n"))
        XCTAssertGreaterThan(keys.count, 100)
    }

    /// Every key has an English value; the Chinese column can be filled
    /// without code changes.
    func testEveryKeyHasEnglish() throws {
        let catalogURL = LocalizationTests.root.appendingPathComponent("App/Resources/Localizable.xcstrings")
        let catalog = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(contentsOf: catalogURL)) as? [String: Any])
        XCTAssertEqual(catalog["sourceLanguage"] as? String, "en")
        let strings = try XCTUnwrap(catalog["strings"] as? [String: [String: Any]])
        for (key, entry) in strings {
            let en = (entry["localizations"] as? [String: Any])?["en"] as? [String: Any]
            let unit = en?["stringUnit"] as? [String: Any]
            XCTAssertEqual(unit?["value"] as? String, key)
        }
    }

    /// The built app finds its texts: a key resolves through the bundle.
    func testBundleResolvesKeys() {
        let value = Bundle.main.localizedString(forKey: "No apps yet", value: "MISSING", table: nil)
        XCTAssertEqual(value, "No apps yet")
    }
}
