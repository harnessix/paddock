import Foundation

/// One instance of an app profile. paddock names them as follows:
///
/// - home folder: `default` for instance 1, `instance-N` for the others
/// - key: `NAME` for instance 1, `NAME--N` for the others (firewall rules,
///   `freeze macvm-KEY`)
/// - label: `NAME` or `NAME N` (the Dock shows `Sample Notes 2 (paddock)`)
///
/// The launcher uses the key as the identity of a sidebar row, so instance 1
/// keeps the profile name and every older call site stays right.
public struct InstanceRef: Hashable, Sendable, Comparable {
    public static let first = 1
    public static let max = 99
    public static let separator = "--"

    public var app: String
    public var number: Int

    public init(app: String, number: Int = InstanceRef.first) {
        self.app = app
        self.number = number
    }

    /// Reads a home folder name. Any home that is not `instance-N` with N in
    /// 2...99 counts as instance 1.
    public init(app: String, home: String?) {
        self.app = app
        self.number = InstanceRef.number(home: home)
    }

    /// Reads a key: `sample-notes` or `sample-notes--2`. A name that itself contains
    /// `--` but no number after the last one stays whole.
    public init(key: String) {
        if let range = key.range(of: InstanceRef.separator, options: .backwards),
           let number = Int(key[range.upperBound...]), number > InstanceRef.first, number <= InstanceRef.max {
            self.app = String(key[..<range.lowerBound])
            self.number = number
        } else {
            self.app = key
            self.number = InstanceRef.first
        }
    }

    public static func number(home: String?) -> Int {
        guard let home, home.hasPrefix("instance-"), let n = Int(home.dropFirst("instance-".count)),
              n > first, n <= max else { return first }
        return n
    }

    public var isFirst: Bool { number == InstanceRef.first }
    public var home: String { isFirst ? "default" : "instance-\(number)" }
    public var key: String { isFirst ? app : app + InstanceRef.separator + "\(number)" }

    /// `--instance N` for every instance but the first, which needs no flag.
    public var flag: [String] { isFirst ? [] : ["--instance", "\(number)"] }

    /// The name a person reads: `Sample Notes` or `Sample Notes 2`.
    public func title(displayName: String?) -> String {
        let base = displayName.flatMap { $0.isEmpty ? nil : $0 } ?? app
        return isFirst ? base : "\(base) \(number)"
    }

    public static func < (a: InstanceRef, b: InstanceRef) -> Bool {
        a.app == b.app ? a.number < b.number : a.app < b.app
    }
}

/// The instances of one app, for the sidebar's disclosure rows.
public struct InstanceGroup: Equatable, Sendable, Identifiable {
    public var app: String
    public var title: String
    public var items: [AppItem]

    public var id: String { app }
    /// More than one instance: the sidebar shows a disclosure row.
    public var isGroup: Bool { items.count > 1 }

    public var countText: UIText { UIText("%@ instances", "\(items.count)") }

    /// Groups the items by app in list order; instances in number order.
    public static func groups(_ items: [AppItem]) -> [InstanceGroup] {
        var order: [String] = []
        var byApp: [String: [AppItem]] = [:]
        for item in items {
            let app = item.instance.app
            if byApp[app] == nil { order.append(app) }
            byApp[app, default: []].append(item)
        }
        return order.map { app in
            let items = byApp[app]!.sorted { $0.instance.number < $1.instance.number }
            let first = items.first
            return InstanceGroup(app: app, title: first?.json.displayName ?? app, items: items)
        }
    }
}

extension AppItem {
    /// The instance this row stands for.
    public var instance: InstanceRef { InstanceRef(app: json.name, home: json.home) }

    /// The sidebar identity: the profile name, or `NAME--N`.
    public var key: String { instance.key }

    /// The name on screen: `Sample Notes` or `Sample Notes 2`.
    public var title: String { instance.title(displayName: json.displayName) }
}
