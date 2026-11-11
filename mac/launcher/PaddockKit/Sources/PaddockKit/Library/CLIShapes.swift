import Foundation

// The JSON that paddock 0.2.x prints for the library, the sessions, the
// setup and the storage, as src/cli writes it. Every field that paddock may
// leave out or set to null is optional, so a newer paddock still decodes.
// `PaddockJSON.decoder` converts the snake_case keys.

// MARK: - app list --json

/// `paddock app list --json`: `{"v":1,"profiles":[...]}`.
public struct ProfileListJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var profiles: [ProfileJSON]
}

/// One `(name, home)` of `app list --json`.
public struct ProfileJSON: Codable, Equatable, Sendable {
    public var name: String
    /// `default` or `instance-N`.
    public var home: String?
    public var bytes: Int64?
    public var added: Bool?
    public var bundle: String?
    public var displayName: String?
    public var version: String?
    public var icon: String?
    /// True when `~/Applications/<Name> (paddock).app` exists.
    public var launcher: Bool?
    /// Why an added app cannot start, paddock's words.
    public var blocked: String?
    /// The session status. paddock sets it for the default home only.
    public var status: ProfileStatusJSON?

    public init(name: String, home: String? = nil, bytes: Int64? = nil, added: Bool? = nil,
                bundle: String? = nil, displayName: String? = nil, version: String? = nil,
                icon: String? = nil, launcher: Bool? = nil, blocked: String? = nil,
                status: ProfileStatusJSON? = nil) {
        self.name = name
        self.home = home
        self.bytes = bytes
        self.added = added
        self.bundle = bundle
        self.displayName = displayName
        self.version = version
        self.icon = icon
        self.launcher = launcher
        self.blocked = blocked
        self.status = status
    }
}

/// The part of paddock's session status that the library reads.
public struct ProfileStatusJSON: Codable, Equatable, Sendable {
    /// `running`, `suspended`, `stopped`, `never_started`.
    public var state: String
    public var pid: Int?
    public var uptimeS: Int64?
    /// `warm`, `cold`, `fresh`, `repair`.
    public var startKind: String?
    /// For `stopped`: `quit with save`, `quit without save`, `crashed`, ...
    public var lastEnd: String?
    /// `warm`, `cold`, `cold: <why>`, `refused: <reason>`.
    public var nextStart: String?
    public var golden: String?
    public var app: StatusAppJSON?
    public var windows: Int?
    public var host: StatusHostJSON?
    public var network: StatusNetworkJSON?
    public var suspendReason: String?

    public init(state: String, pid: Int? = nil, uptimeS: Int64? = nil, startKind: String? = nil,
                lastEnd: String? = nil, nextStart: String? = nil, golden: String? = nil,
                app: StatusAppJSON? = nil, windows: Int? = nil, host: StatusHostJSON? = nil,
                network: StatusNetworkJSON? = nil, suspendReason: String? = nil) {
        self.state = state
        self.pid = pid
        self.uptimeS = uptimeS
        self.startKind = startKind
        self.lastEnd = lastEnd
        self.nextStart = nextStart
        self.golden = golden
        self.app = app
        self.windows = windows
        self.host = host
        self.network = network
        self.suspendReason = suspendReason
    }
}

public struct StatusAppJSON: Codable, Equatable, Sendable {
    public var name: String?
    public var version: String?
    public var copied: String?
    public var running: Bool?
    public var windows: Int?
}

public struct StatusHostJSON: Codable, Equatable, Sendable {
    public var profileBytes: Int64?
    public var dataBytes: Int64?
    public var stateBytes: Int64?
    public var diskFree: Int64?

    public init(profileBytes: Int64? = nil, dataBytes: Int64? = nil, stateBytes: Int64? = nil, diskFree: Int64? = nil) {
        self.profileBytes = profileBytes
        self.dataBytes = dataBytes
        self.stateBytes = stateBytes
        self.diskFree = diskFree
    }
}

public struct StatusNetworkJSON: Codable, Equatable, Sendable {
    /// Empty while the app does not run.
    public var mode: String?
}

// MARK: - app add --json

/// `paddock app add PATH [--dry-run] --json`: one shape for the precheck, the
/// add and every refusal.
public struct AddReportJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var path: String?
    public var name: String?
    public var displayName: String?
    public var version: String?
    public var build: String?
    public var bundleId: String?
    public var icon: String?
    public var bytes: Int64?
    public var appStore: Bool?
    /// The profile name when the bundle is in paddock already.
    public var existing: String?
    /// `missing`, `unreadable`, a build step, or `sealed`.
    public var golden: String?
    public var space: AddSpaceJSON?
    public var memory: AddMemoryJSON?
    public var refusals: [AddRefusalJSON]?
    public var ok: Bool?
    public var dryRun: Bool?
    public var added: Bool?
    /// A failure that is not a refusal: `{"v":1,"error":"..."}`.
    public var error: String?
}

public struct AddSpaceJSON: Codable, Equatable, Sendable {
    public var need: Int64
    public var free: Int64
    public var ok: Bool
}

public struct AddMemoryJSON: Codable, Equatable, Sendable {
    public var mib: Int64?
    public var defaultMib: Int64?
    public var maxMib: Int64?
}

public struct AddRefusalJSON: Codable, Equatable, Sendable {
    /// `not_a_bundle`, `app_store`, `bad_name`, `name_taken`, `bad_memory`,
    /// `no_golden`, `low_space`.
    public var code: String
    /// paddock's line, shown verbatim.
    public var text: String

    public init(code: String, text: String) {
        self.code = code
        self.text = text
    }
}

// MARK: - app remove --json

/// `paddock app remove NAME [--delete-data] --yes --json`.
public struct RemoveReportJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var name: String?
    public var freed: Int64?
    /// The trash folder of the data, or nil with `--delete-data`.
    public var trash: String?
    public var trashBytes: Int64?
    public var launcher: String?
    public var config: Bool?
    public var error: String?
}

// MARK: - macvm status --json

/// `paddock macvm status --json` as paddock 0.2.x prints it.
public struct MacvmStatusV1JSON: Codable, Equatable, Sendable {
    public var v: Int
    public var dir: String?
    public var golden: GoldenV1JSON
    public var prev: GoldenPrevJSON?
    public var hostVersion: String?
    public var profiles: [MacvmProfileJSON]?
    public var slots: [String?]?
    public var paused: Bool?
    public var setup: SetupStateJSON?
}

public struct GoldenV1JSON: Codable, Equatable, Sendable {
    /// `missing`, `unreadable`, a build step, or `sealed`.
    public var state: String
    public var version: String?
    public var build: String?
    /// The seal time, RFC 3339, when sealed.
    public var sealed: String?
    public var bytes: Int64?
    public var error: String?
    public var offlineFailed: String?
}

public struct GoldenPrevJSON: Codable, Equatable, Sendable {
    public var version: String
    public var removable: Bool?
}

public struct MacvmProfileJSON: Codable, Equatable, Sendable {
    public var app: String
    public var home: String
    public var bytes: Int64?
    public var state: String?
    public var lastEnd: String?
}

public struct SetupStateJSON: Codable, Equatable, Sendable {
    public var running: Bool?
    public var paused: Bool?
    public var last: SetupLineJSON?
}

/// One `setup` line of `macvm setup --json` and of the event stream.
public struct SetupLineJSON: Codable, Equatable, Sendable {
    public var t: Date?
    /// `fetch`, `seed`, `install`, `guided`, `trust`, `configure`, `seal`, `sealed`.
    public var step: String
    public var bytes: Int64?
    public var total: Int64?
    public var rate: Double?
    public var paused: Bool?
    /// paddock's failure line.
    public var failed: String?
    /// True when the user stopped the guided step.
    public var stopped: Bool?
    /// A text line of the setup helper.
    public var line: String?

    public init(t: Date? = nil, step: String, bytes: Int64? = nil, total: Int64? = nil, rate: Double? = nil,
                paused: Bool? = nil, failed: String? = nil, stopped: Bool? = nil, line: String? = nil) {
        self.t = t
        self.step = step
        self.bytes = bytes
        self.total = total
        self.rate = rate
        self.paused = paused
        self.failed = failed
        self.stopped = stopped
        self.line = line
    }
}

/// `paddock macvm setup --pause-download --json` and `--resume --json`
/// while a build runs.
public struct SetupControlJSON: Codable, Equatable, Sendable {
    public var v: Int
    /// `requested`, `no_build`, `past_download`, `resumed`.
    public var result: String?
    public var step: String?
    public var text: String?
    public var error: String?
}

// MARK: - du --json

/// `paddock du --json`. The launcher reads the `macvm` part only.
public struct DuReportJSON: Codable, Equatable, Sendable {
    public var apiVersion: String?
    public var anyRunning: Bool?
    public var macvm: MacvmDuJSON?
}

public struct MacvmDuJSON: Codable, Equatable, Sendable {
    public var root: String
    public var bytes: Int64
    public var golden: Int64?
    public var goldenPrev: Int64?
    public var goldenNew: Int64?
    public var ipsw: Int64?
    public var agent: Int64?
    public var trash: Int64?
    public var shims: Int64?
    public var profiles: [MacvmDuProfileJSON]?
}

public struct MacvmDuProfileJSON: Codable, Equatable, Sendable {
    public var app: String
    public var instance: Int
    public var path: String?
    /// `running`, `suspended`, `stopped`.
    public var state: String
    public var bytes: Int64
    public var disks: Int64?
    public var savedState: Int64?
    public var apps: Int64?
    public var archives: Int64?
    public var backups: Int64?
    /// Unix seconds.
    public var lastUsed: Int64?
}

// MARK: - gc --json

/// `paddock gc [--yes] --json`.
public struct GcReportJSON: Codable, Equatable, Sendable {
    public var apiVersion: String?
    public var dryRun: Bool?
    public var items: [GcItemJSON]
    public var removed: [GcRemovedJSON]?
    public var notes: [String]?
}

public struct GcItemJSON: Codable, Equatable, Sendable {
    public var what: String
    /// `auto` or `consent`.
    public var tier: String
    public var bytes: Int64
    public var reason: String?
    public var warning: String?
    public var needs: [String]?
    public var action: GcActionJSON?

    public init(what: String, tier: String, bytes: Int64, reason: String? = nil, warning: String? = nil,
                needs: [String]? = nil, action: GcActionJSON? = nil) {
        self.what = what
        self.tier = tier
        self.bytes = bytes
        self.reason = reason
        self.warning = warning
        self.needs = needs
        self.action = action
    }
}

public struct GcActionJSON: Codable, Equatable, Sendable {
    /// `image`, `buildcache`, `path`, `worktree`, `kv`, `view`, `caches`,
    /// `macvm`, `buildoutput`.
    public var kind: String
    public var dir: String?

    public init(kind: String, dir: String? = nil) {
        self.kind = kind
        self.dir = dir
    }
}

public struct GcRemovedJSON: Codable, Equatable, Sendable {
    public var what: String
    public var bytes: Int64?
    public var error: String?
    public var note: String?
}
