import Foundation

/// The errors a normal user can act on. Each has a title (what did not
/// happen), a body (why, and what to do), one primary button and, for most,
/// `Show Log` as the secondary button.
public enum LauncherError: Equatable, Sendable {
    case notInstalled
    case tooOld(have: String, need: String)
    case tooNew(have: String)
    case twoRunning(app: String, running: [String])
    case lowDiskAtStart(app: String, needBytes: Int64, freeBytes: Int64)
    case appChanged(app: String)
    case appCopyFailed(app: String, line: String)
    case crashed(app: String, crashRow: String?)
    case agentAway
    case goldenRemoved
    case streamLost
    /// A failed command with paddock's own line.
    case command(title: UIText, line: String)

    /// Where the error shows.
    public enum Placement: Equatable, Sendable {
        /// An app-level `NSAlert`.
        case alert
        /// The error shape in place in the window.
        case inPlace
        /// An info row, not an error.
        case info
        /// A banner in the Overview tab.
        case banner
        /// A one-line status line.
        case statusLine
        /// The sidebar row shows `blocked`.
        case row
    }

    public enum Primary: Equatable, Sendable {
        case checkAgain
        case copyCommand
        case stopPicker
        case manageStorage
        case showLog
        case startAgain
        case build
        case refresh
        case none

        public var title: UIText? {
            switch self {
            case .checkAgain: return UIText("Check Again")
            case .copyCommand: return UIText("Copy Command")
            case .stopPicker: return UIText("Stop…")
            case .manageStorage: return UIText("Manage Storage…")
            case .showLog: return UIText("Show Log")
            case .startAgain: return UIText("Start Again")
            case .build: return UIText("Build…")
            case .refresh: return UIText("Refresh")
            case .none: return nil
            }
        }
    }

    public var placement: Placement {
        switch self {
        case .notInstalled, .tooOld, .tooNew: return .alert
        case .appChanged: return .info
        case .agentAway: return .banner
        case .streamLost: return .statusLine
        case .goldenRemoved: return .row
        default: return .inPlace
        }
    }

    public var symbol: String {
        switch self {
        case .appChanged, .agentAway: return "info.circle"
        case .streamLost: return "wifi.exclamationmark"
        case .goldenRemoved: return "nosign"
        default: return "exclamationmark.triangle.fill"
        }
    }

    public var title: UIText? {
        switch self {
        case .notInstalled: return UIText("paddock is not installed")
        case .tooOld: return UIText("Update paddock")
        case .tooNew: return UIText("Update paddock for Mac")
        case .twoRunning: return UIText("Two apps are already running")
        case .lowDiskAtStart(let app, _, _): return UIText("Not enough free space to start %@", app)
        case .appChanged: return nil
        case .appCopyFailed(let app, _): return UIText("%@ runs from the previous copy", app)
        case .crashed(let app, _): return UIText("%@ stopped unexpectedly", app)
        case .agentAway: return nil
        case .goldenRemoved: return nil
        case .streamLost: return UIText("Live updates paused")
        case .command(let title, _): return title
        }
    }

    public var body: UIText? {
        switch self {
        case .notInstalled:
            return UIText("paddock for Mac needs the paddock command. Install it with Homebrew, then check again.")
        case .tooOld(let have, let need):
            return UIText("This version of paddock for Mac needs paddock %@ or newer. You have %@.", need, have)
        case .tooNew(let have):
            return UIText("This version of paddock for Mac is too old for paddock %@. Update paddock for Mac.", have)
        case .twoRunning(let app, _):
            return UIText("macOS allows two sandboxed apps at a time. Stop one to start %@.", app)
        case .lowDiskAtStart(_, let need, let free):
            return UIText("Starting needs %@, %@ is free.", Format.gb(need), Format.gb(free))
        case .appChanged(let app):
            return UIText("%@ was updated on this Mac. The next start copies the new version, about 1 min.", app)
        case .appCopyFailed(_, let line):
            return UIText(key: "%@", args: [line])
        case .crashed:
            return UIText("Its saved state was not written. The next start is cold.")
        case .agentAway:
            return UIText("paddock lost contact with the app's helper. Menus and clipboard pause until it is back.")
        case .goldenRemoved:
            return UIText("Build the base image to start this app.")
        case .streamLost:
            return UIText("Live updates paused")
        case .command(_, let line):
            return UIText(key: "%@", args: [line])
        }
    }

    /// A second line under the body: the crash row of paddock's failure table.
    public var detail: String? {
        if case .crashed(_, let row) = self { return row }
        return nil
    }

    public var primary: Primary {
        switch self {
        case .notInstalled: return .checkAgain
        case .tooOld, .tooNew: return .copyCommand
        case .twoRunning: return .stopPicker
        case .lowDiskAtStart: return .manageStorage
        case .appChanged, .agentAway: return .none
        case .appCopyFailed: return .showLog
        case .crashed: return .startAgain
        case .goldenRemoved: return .build
        case .streamLost: return .refresh
        case .command: return .showLog
        }
    }

    /// True when the error shows `Show Log` as its secondary button.
    public var showsLogButton: Bool {
        switch self {
        case .appChanged, .agentAway, .streamLost, .goldenRemoved, .appCopyFailed, .command: return false
        default: return true
        }
    }

    /// The command the user copies for the install and update errors.
    public var command: String? {
        switch self {
        case .notInstalled: return ShellCommands.brewInstall
        case .tooOld: return ShellCommands.brewUpgrade
        case .tooNew: return ShellCommands.brewUpgradeLauncher
        default: return nil
        }
    }
}

/// The terminal commands the launcher shows with a Copy button. It never
/// runs them itself.
public enum ShellCommands {
    public static let brewInstall = "brew install harnessix/tap/paddock"
    public static let brewUpgrade = "brew upgrade paddock"
    public static let brewUpgradeLauncher = "brew upgrade --cask paddock-mac"
    public static let brewUninstall = "brew uninstall paddock"
    public static func allHostsRule(app: String) -> String {
        "paddock app rules \(app) add '*' --yes-all-hosts"
    }
}
