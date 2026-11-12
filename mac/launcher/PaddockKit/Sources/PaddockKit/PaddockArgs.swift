import Foundation

public enum NetworkMode: String, CaseIterable, Equatable, Sendable, Identifiable {
    case ask, allowlist, none

    public var id: String { rawValue }

    public var title: UIText {
        switch self {
        case .ask: return UIText("Ask")
        case .allowlist: return UIText("Allow list only")
        case .none: return UIText("No network")
        }
    }
}

public enum Answer: String, CaseIterable, Equatable, Sendable {
    case deny
    case allowOnce = "allow-once"
    case allowSession = "allow-session"
    case allowAlways = "allow-always"
    case denyAlways = "deny-always"

    public var flag: String { "--" + rawValue }

    public var title: UIText {
        switch self {
        case .deny: return UIText("Deny")
        case .allowOnce: return UIText("Allow once")
        case .allowSession: return UIText("Allow this session")
        case .allowAlways: return UIText("Always allow")
        case .denyAlways: return UIText("Always deny")
        }
    }
}

public enum ShareMode: String, Equatable, Sendable, CaseIterable, Identifiable {
    case ro, rw

    public var id: String { rawValue }

    public var title: UIText {
        switch self {
        case .ro: return UIText("Read only")
        case .rw: return UIText("Read and write")
        }
    }
}

public enum RuleAnswer: String, Equatable, Sendable, CaseIterable, Identifiable {
    case allow, deny
    public var id: String { rawValue }
    public var title: UIText { self == .allow ? UIText("Allow") : UIText("Deny") }
}

public enum RuleScope: String, Equatable, Sendable, CaseIterable, Identifiable {
    case always, session
    public var id: String { rawValue }
    public var title: UIText { self == .always ? UIText("Always") : UIText("This session") }
}

/// The argument lists of every paddock call the launcher makes. One place,
/// so the service and the "Show the paddock command" row agree.
public enum PaddockArgs {
    public static let version = ["--version"]
    /// The read-only query of `LauncherProtocol`. It must be the only word.
    public static let launcherProtocol = ["--launcher-protocol"]
    public static let macvmStatus = ["macvm", "status", "--json"]
    public static let doctor = ["doctor", "--json"]
    public static let appList = ["app", "list", "--json"]
    public static let approvals = ["approvals", "--json"]
    public static let events = ["events", "--follow", "--json"]
    public static let macvmRemove = ["macvm", "remove", "--json"]
    public static let macvmLatest = ["macvm", "latest", "--json"]
    public static let setupPause = ["macvm", "setup", "--pause-download"]
    public static let setupShowWindow = ["macvm", "setup", "--show-window"]
    public static let deleteRestoreImage = ["macvm", "clean", "--ipsw", "--json"]
    public static let doctorReport = ["doctor", "--report", "--no-hosts"]

    public static func appStatus(_ app: String) -> [String] { ["app", "status", app, "--json"] }
    public static func run(_ app: String) -> [String] { ["app", "run", app, "--json"] }
    public static func stop(_ app: String) -> [String] { ["app", "stop", app] }
    public static func open(_ app: String) -> [String] { ["app", "open", app] }
    public static func answer(_ id: String, _ answer: Answer) -> [String] { ["approvals", "answer", id, answer.flag] }
    public static func pausedDenyAll(_ app: String) -> [String] { ["approvals", "paused", app, "--deny-all"] }
    /// `window` or `center`: where paddock's question panel opens.
    public static func panelPosition(_ value: String) -> [String] { ["approvals", "panel-position", value] }
    public static func pausedOneByOne(_ app: String) -> [String] { ["approvals", "paused", app, "--one-by-one"] }

    public static func setup(dir: String?, ipsw: String?, resume: Bool, rebuild: Bool = false) -> [String] {
        var args = ["macvm", "setup", "--json"]
        if let dir { args += ["--dir", dir] }
        if let ipsw { args += ["--ipsw", ipsw] }
        if resume { args.append("--resume") }
        if rebuild { args.append("--rebuild") }
        return args
    }

    public static func precheck(_ path: String) -> [String] { ["app", "add", path, "--dry-run", "--json"] }

    public static func add(_ path: String, name: String, memoryGB: Int, network: NetworkMode) -> [String] {
        ["app", "add", path, "--name", name, "--memory", "\(memoryGB)", "--network", network.rawValue, "--json"]
    }

    public static func install(_ app: String) -> [String] { ["app", "install", app] }

    public static func remove(_ app: String, deleteData: Bool) -> [String] {
        deleteData ? ["app", "remove", app, "--delete-data", "--json"] : ["app", "remove", app, "--json"]
    }

    public static func clearSavedState(_ app: String) -> [String] { ["app", "clear-state", app, "--json"] }
    public static func network(_ app: String, _ mode: NetworkMode) -> [String] { ["app", "network", app, mode.rawValue] }

    public static func shareAdd(_ app: String, path: String, rw: Bool, unprotected: Bool) -> [String] {
        var args = ["app", "share", app, "add", path]
        if rw { args.append("--rw") }
        if unprotected { args.append("--unprotected") }
        return args + ["--json"]
    }

    public static func shareRemove(_ app: String, path: String) -> [String] { ["app", "share", app, "remove", path, "--json"] }

    public static func shareMode(_ app: String, path: String, mode: ShareMode) -> [String] {
        ["app", "share", app, "mode", path, mode.rawValue, "--json"]
    }

    public static func downloads(_ app: String, path: String) -> [String] { ["app", "share", app, "downloads", path, "--json"] }

    public static func ruleAdd(_ app: String, host: String, answer: RuleAnswer, scope: RuleScope) -> [String] {
        ["app", "rules", app, "add", host, "--" + answer.rawValue, "--" + scope.rawValue, "--json"]
    }

    public static func ruleRemove(_ app: String, host: String) -> [String] { ["app", "rules", app, "remove", host, "--json"] }

    public static func clipboard(_ app: String, toApp: Bool, fromApp: Bool) -> [String] {
        ["app", "clipboard", app, "--to-app", toApp ? "on" : "off", "--from-app", fromApp ? "on" : "off"]
    }

    public static func device(_ app: String, name: String, on: Bool) -> [String] {
        ["app", "device", app, name, on ? "on" : "off"]
    }

    public static func log(_ app: String) -> [String] { ["app", "log", app, "--events", "--tail", "500"] }
    public static func logFollow(_ app: String) -> [String] { ["app", "log", app, "--events", "--follow"] }

    /// The command line as the user would type it, for the support row.
    public static func display(_ args: [String]) -> String {
        (["paddock"] + args.map(quote)).joined(separator: " ")
    }

    static func quote(_ word: String) -> String {
        let safe = word.allSatisfy { $0.isLetter || $0.isNumber || "-_./~=:@".contains($0) }
        return safe && !word.isEmpty ? word : "'" + word.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}
