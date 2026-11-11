import Foundation

/// Where paddock for Mac itself runs from, and whether it offers to move
/// itself into ~/Applications. The file work is in the app; the rules are
/// here, so they run in the unit tests.
public enum SelfInstall {
    public static let bundleName = "paddock for Mac.app"

    public enum Place: Equatable, Sendable {
        /// `/Applications` or a folder inside it.
        case applications
        /// `~/Applications` or a folder inside it.
        case userApplications
        /// macOS runs a quarantined app from a random read-only path.
        case translocated
        /// A mounted disk image or another read-only volume.
        case readOnlyVolume
        /// Downloads, Desktop or any other folder.
        case elsewhere
        /// Xcode's build folder: never offer a move there.
        case buildFolder
    }

    public static func place(bundlePath: String, home: String, readOnlyVolume: Bool) -> Place {
        let path = bundlePath.hasSuffix("/") ? String(bundlePath.dropLast()) : bundlePath
        if path.contains("/AppTranslocation/") { return .translocated }
        if path.contains("/DerivedData/") || path.contains("/Build/Products/") { return .buildFolder }
        if path.hasPrefix("/Applications/") { return .applications }
        let user = (home.hasSuffix("/") ? String(home.dropLast()) : home) + "/Applications/"
        if !home.isEmpty, path.hasPrefix(user) { return .userApplications }
        if readOnlyVolume { return .readOnlyVolume }
        return .elsewhere
    }

    /// The launch question shows once, outside an Applications folder.
    public static func shouldOffer(_ place: Place, declined: Bool) -> Bool {
        guard !declined else { return false }
        switch place {
        case .translocated, .readOnlyVolume, .elsewhere: return true
        case .applications, .userApplications, .buildFolder: return false
        }
    }

    /// The original can go after the copy only from a writable folder.
    public static func removesOriginal(_ place: Place) -> Bool { place == .elsewhere }

    public static func target(home: String) -> String {
        (home.hasSuffix("/") ? String(home.dropLast()) : home) + "/Applications/" + bundleName
    }

    public enum Decision: Equatable, Sendable {
        /// Nothing there: copy.
        case copy
        /// An older copy there: replace it (it goes to the Trash first).
        case replace
        /// The same or a newer version there: open that one.
        case openExisting
    }

    public static func decision(existing: String?, mine: String) -> Decision {
        guard let existing else { return .copy }
        return compare(existing, mine) < 0 ? .replace : .openExisting
    }

    /// Compares dotted versions number by number: -1, 0 or 1.
    public static func compare(_ a: String, _ b: String) -> Int {
        let x = a.split(separator: ".").map { Int($0) ?? 0 }
        let y = b.split(separator: ".").map { Int($0) ?? 0 }
        for i in 0..<max(x.count, y.count) {
            let l = i < x.count ? x[i] : 0
            let r = i < y.count ? y[i] : 0
            if l != r { return l < r ? -1 : 1 }
        }
        return 0
    }

    public static let question = UIText("Move paddock for Mac to your Applications folder?")
    public static let questionBody = UIText("It then opens from Launchpad and Spotlight, and keeps working after you eject the disk image or empty Downloads. paddock for Mac copies itself into the Applications folder of your home folder.")

    public static func locationLine(_ place: Place) -> UIText {
        switch place {
        case .applications, .userApplications: return UIText("In your Applications folder")
        case .translocated, .readOnlyVolume: return UIText("Runs from a disk image or a temporary copy")
        case .elsewhere: return UIText("Not in an Applications folder")
        case .buildFolder: return UIText("A development build")
        }
    }
}
