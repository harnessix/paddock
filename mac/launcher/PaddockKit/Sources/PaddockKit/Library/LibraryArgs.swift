import Foundation

/// The argument lists of the library, session, setup and storage calls, in
/// the form paddock 0.2.x takes them (src/cli/app.rs, app_add.rs, macvm.rs,
/// mod.rs). One place, so the service and the support row agree.
public enum LibraryArgs {
    /// `app run` has no `--json`. Instance 1 needs no flag.
    public static func run(_ ref: InstanceRef) -> [String] { ["app", "run", ref.app] + ref.flag }

    /// `app run NAME --new`: paddock picks the lowest free instance number.
    public static func runNew(_ app: String) -> [String] { ["app", "run", app, "--new"] }

    public static func stop(_ ref: InstanceRef) -> [String] { ["app", "stop", ref.app] + ref.flag }

    /// Asked of paddock only when `Capabilities.suspend` is true.
    public static func suspend(_ ref: InstanceRef) -> [String] { ["app", "suspend", ref.app] + ref.flag + ["--json"] }

    /// `app add PATH --dry-run --json`: the review of the add sheet.
    public static func review(_ path: String) -> [String] { ["app", "add", path, "--dry-run", "--json"] }

    /// `--memory` takes a size such as `6G`.
    public static func add(_ path: String, name: String, memoryGB: Int) -> [String] {
        ["app", "add", path, "--name", name, "--memory", "\(memoryGB)G", "--json"]
    }

    /// `--json` needs `--yes`.
    public static func remove(_ app: String, deleteData: Bool) -> [String] {
        deleteData ? ["app", "remove", app, "--delete-data", "--yes", "--json"] : ["app", "remove", app, "--yes", "--json"]
    }

    /// `--json` first, so the older `macvm setup --json` fixtures still match.
    public static func setup(ipsw: String?, resume: Bool) -> [String] {
        var args = ["macvm", "setup", "--json", "--yes"]
        if let ipsw { args += ["--ipsw", ipsw] }
        if resume { args.append("--resume") }
        return args
    }

    public static let setupPause = ["macvm", "setup", "--pause-download", "--json"]
    public static let setupResume = ["macvm", "setup", "--resume", "--json"]
    public static let update = ["macvm", "update", "--yes"]
    public static let macvmRemove = ["macvm", "remove", "--yes", "--json"]
    public static let appHelp = ["app", "--help"]

    public static let du = ["du", "--json"]

    /// A dry run unless `apply`. `includeLogins` adds the consent tier of
    /// whole unused profiles, whose login is lost.
    public static func gc(apply: Bool, includeLogins: Bool) -> [String] {
        var args = ["gc"]
        if apply { args.append("--yes") }
        if includeLogins { args += ["--homes", "--include-app-logins"] }
        return args + ["--json"]
    }
}

/// What this paddock can do beyond the 0.2.2 base, read from the
/// subcommand list of `paddock app --help`.
public struct Capabilities: Equatable, Sendable {
    public var suspend = false

    public init(suspend: Bool = false) {
        self.suspend = suspend
    }

    /// clap prints each subcommand as a line `  NAME  text`.
    public static func parse(appHelp text: String) -> Capabilities {
        var caps = Capabilities()
        for line in text.split(separator: "\n") {
            let words = line.split(separator: " ", omittingEmptySubsequences: true)
            guard line.hasPrefix("  "), let first = words.first else { continue }
            if first == "suspend" { caps.suspend = true }
        }
        return caps
    }
}
