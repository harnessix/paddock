import Foundation

/// A user-visible text: a key of Localizable.xcstrings plus the values for
/// its `%@` placeholders. PaddockKit builds texts, the app localizes them.
/// The key is the English text, so `english` needs no catalog.
public struct UIText: Equatable, Hashable, Sendable, CustomStringConvertible {
    public let key: String
    public let args: [String]

    public init(_ key: String, _ args: String...) {
        self.key = key
        self.args = args
    }

    public init(key: String, args: [String]) {
        self.key = key
        self.args = args
    }

    /// The English text with the placeholders filled in.
    public var english: String { UIText.fill(key, args) }

    public var description: String { english }

    /// Replaces each `%@` of `format` with the next value of `args`.
    public static func fill(_ format: String, _ args: [String]) -> String {
        var out = ""
        var rest = Substring(format)
        var values = args.makeIterator()
        while let range = rest.range(of: "%@") {
            out += rest[rest.startIndex..<range.lowerBound]
            out += values.next() ?? ""
            rest = rest[range.upperBound...]
        }
        out += rest
        return out
    }
}

/// Number and time formats of the copy rules: sizes in GB with one
/// decimal, times in minutes, no seconds above one minute.
public enum Format {
    static let gigabyte = 1_000_000_000.0

    /// `25.3 GB`. Decimal gigabytes, as Finder shows them.
    public static func gb(_ bytes: Int64) -> String {
        let value = Double(bytes) / gigabyte
        return String(format: "%.1f GB", value)
    }

    /// `25.3`, without the unit, for "6.2 of 13.1 GB".
    public static func gbNumber(_ bytes: Int64) -> String {
        String(format: "%.1f", Double(bytes) / gigabyte)
    }

    /// Whole gigabytes for memory: `16 GB`.
    public static func memoryGB(_ bytes: Int64) -> String {
        let gib = Double(bytes) / (1024 * 1024 * 1024)
        return "\(Int(gib.rounded())) GB"
    }

    /// Whole gigabytes without the unit: `16`.
    public static func memoryNumber(_ bytes: Int64) -> String {
        "\(Int((Double(bytes) / (1024 * 1024 * 1024)).rounded()))"
    }

    /// The age of a past moment: `just now`, `3 min ago`, `2 h ago`, `4 d ago`.
    public static func age(since date: Date, now: Date) -> UIText {
        let seconds = max(0, now.timeIntervalSince(date))
        if seconds < 60 { return UIText("just now") }
        let minutes = Int(seconds / 60)
        if minutes < 60 { return UIText("%@ min ago", "\(minutes)") }
        let hours = minutes / 60
        if hours < 48 { return UIText("%@ h ago", "\(hours)") }
        return UIText("%@ d ago", "\(hours / 24)")
    }

    /// The time left of a job: `less than a minute left`, `9 min left`.
    public static func timeLeft(seconds: Double) -> UIText {
        if seconds < 60 { return UIText("less than a minute left") }
        return UIText("%@ min left", "\(Int((seconds / 60).rounded(.up)))")
    }

    /// `~/Projects` for a path inside the home folder.
    public static func tildePath(_ path: String, home: String) -> String {
        guard !home.isEmpty else { return path }
        if path == home { return "~" }
        let prefix = home.hasSuffix("/") ? home : home + "/"
        if path.hasPrefix(prefix) { return "~/" + path.dropFirst(prefix.count) }
        return path
    }

    /// `12:04` in 24-hour time, for the Changes list.
    public static func clock(_ date: Date, timeZone: TimeZone = .current) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let parts = calendar.dateComponents([.hour, .minute], from: date)
        return String(format: "%02d:%02d", parts.hour ?? 0, parts.minute ?? 0)
    }

    /// `3 Nov` for the base image build date.
    public static func dayMonth(_ date: Date, timeZone: TimeZone = .current) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let parts = calendar.dateComponents([.day, .month], from: date)
        let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                      "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        let month = months[max(0, min(11, (parts.month ?? 1) - 1))]
        return "\(parts.day ?? 1) \(month)"
    }
}
