import Foundation

/// Sorts what the user drops on the window or the Dock icon: app bundles
/// go to the review sheet one after another, anything else is refused with
/// one line.
public struct DropIntake: Equatable, Sendable {
    public var apps: [String]
    public var rejected: [String]

    /// More apps than this in one drop is a mistake: the rest is refused.
    public static let maxApps = 10

    public static func sort(_ paths: [String]) -> DropIntake {
        var apps: [String] = []
        var rejected: [String] = []
        for raw in paths {
            let path = raw.hasSuffix("/") ? String(raw.dropLast()) : raw
            if path.lowercased().hasSuffix(".app"), !apps.contains(path), apps.count < maxApps {
                apps.append(path)
            } else if !apps.contains(path) {
                rejected.append(path)
            }
        }
        return DropIntake(apps: apps, rejected: rejected)
    }
}

/// The apps waiting for the review sheet, in drop order.
public struct AddQueue: Equatable, Sendable {
    public private(set) var paths: [String] = []
    /// How many the current run of drops had, for `App 2 of 3`.
    public private(set) var total = 0
    public private(set) var done = 0

    public init() {}

    public var current: String? { paths.first }
    public var isEmpty: Bool { paths.isEmpty }

    /// `App 2 of 3`, or nil for a single app.
    public var position: UIText? {
        guard total > 1, current != nil else { return nil }
        return LibraryCopy.queue(done + 1, of: total)
    }

    /// Adds the apps that are not queued yet.
    public mutating func enqueue(_ new: [String]) {
        if paths.isEmpty {
            total = 0
            done = 0
        }
        for path in new where !paths.contains(path) {
            paths.append(path)
            total += 1
        }
    }

    /// The sheet for the current app closed: added or cancelled.
    public mutating func advance() {
        guard !paths.isEmpty else { return }
        paths.removeFirst()
        done += 1
        if paths.isEmpty {
            total = 0
            done = 0
        }
    }

    /// `Cancel All`.
    public mutating func clear() {
        paths = []
        total = 0
        done = 0
    }
}
