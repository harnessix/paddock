import Foundation

// The JSON shapes of `paddock ... --json`. Keys are snake_case in JSON and
// camelCase here (`PaddockJSON.decoder` converts them). Every top-level
// object carries `v`, the schema version. Unknown fields are ignored, so a
// newer paddock that adds fields still decodes.

public enum PaddockJSON {
    public static var decoder: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }

    /// The schema version the launcher reads. A higher `v` still decodes
    /// as long as the fields below stay.
    public static let schemaVersion = 1
}

// MARK: - Base image (golden)

public struct GoldenJSON: Codable, Equatable, Sendable {
    /// `none`, `building`, `failed`, `ready`, `needs_update`.
    public var state: String
    public var version: String?
    public var built: Date?
    public var sizeBytes: Int64?
    public var dir: String?
    public var hostVersion: String?
    public var appsUsing: Int?
    public var logPath: String?
    public var setup: SetupJSON?
}

public struct SetupJSON: Codable, Equatable, Sendable {
    /// `download`, `install`, `guided`, `seal`, `done`.
    public var step: String
    public var bytes: Int64?
    public var total: Int64?
    /// Bytes per second, as paddock measures it.
    public var rate: Double?
    public var paused: Bool?
    public var time: Date?
    public var failed: SetupFailureJSON?

    public init(step: String, bytes: Int64? = nil, total: Int64? = nil, rate: Double? = nil,
                paused: Bool? = nil, time: Date? = nil, failed: SetupFailureJSON? = nil) {
        self.step = step
        self.bytes = bytes
        self.total = total
        self.rate = rate
        self.paused = paused
        self.time = time
        self.failed = failed
    }
}

public struct SetupFailureJSON: Codable, Equatable, Sendable {
    /// `no_space`, `download_interrupted`, `unsupported_image`,
    /// `wrong_password` or another code.
    public var code: String
    /// paddock's reason line, shown verbatim.
    public var reason: String
}

public struct DiskJSON: Codable, Equatable, Sendable {
    public var freeBytes: Int64
    public var totalBytes: Int64?
    public var usedByPaddockBytes: Int64?
    public var ipswBytes: Int64?
    public var trashBytes: Int64?
    public var trashDir: String?
}

public struct MacvmStatusJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var golden: GoldenJSON
    public var disk: DiskJSON?
    public var logDir: String?
}

// MARK: - Apps

public struct AppSizesJSON: Codable, Equatable, Sendable {
    public var appCopy: Int64
    public var data: Int64
    public var savedState: Int64

    public init(appCopy: Int64, data: Int64, savedState: Int64) {
        self.appCopy = appCopy
        self.data = data
        self.savedState = savedState
    }

    public var total: Int64 { appCopy + data + savedState }
}

public struct AppJSON: Codable, Equatable, Sendable, Identifiable {
    public var name: String
    public var bundlePath: String?
    /// `stopped`, `saved`, `starting`, `running`, `stopping`, `error`, `blocked`.
    public var state: String
    /// For `starting`: `cloning`, `booting`, `copying_app`, `opening`, `resuming`.
    public var phase: String?
    public var since: Date?
    public var windows: Int?
    /// The short reason of `error` or `blocked`, paddock's words.
    public var reason: String?
    /// For `blocked`: `app_store`, `bundle_missing`, `low_disk`, `no_golden`.
    public var blockCode: String?
    /// The `Next start` line, shown verbatim.
    public var nextStart: String?
    public var memoryBytes: Int64?
    public var hostMemoryBytes: Int64?
    public var sizes: AppSizesJSON?
    /// `ask`, `allowlist`, `none`.
    public var network: String?
    public var openQuestions: Int?
    public var ruleCount: Int?
    /// The `Base image` line, verbatim: `macOS 15.1 (sealed)`.
    public var golden: String?
    /// True when the host app changed since the last copy.
    public var appChanged: Bool?
    /// False while the guest agent is away.
    public var agentConnected: Bool?
    /// The crash row of paddock's failure table after an unexpected stop.
    public var crash: String?
    /// paddock's line after a failed app copy. The app runs the old copy.
    public var copyFailed: String?
    /// The home folder of the instance: `default` or `instance-N`.
    public var home: String?
    /// The app's own name from its bundle: `Sample Notes` for `sample-notes`.
    public var displayName: String?
    /// The bundle version of the host app.
    public var version: String?
    /// The disk use of the profile instance.
    public var bytes: Int64?
    /// True when `~/Applications/<Name> (paddock).app` exists.
    public var launcher: Bool?
    /// Why paddock suspended the session, for `suspended`.
    public var suspendReason: String?

    public var id: String { InstanceRef(app: name, home: home).key }
}

public struct AppListJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var apps: [AppJSON]
}

public struct ShareJSON: Codable, Equatable, Sendable, Identifiable {
    public var path: String
    /// `ro` or `rw`.
    public var mode: String
    public var lastUsed: Date?
    public var openHandles: Int?
    public var guestPath: String?
    /// `live`, `next_start` or `reopen`.
    public var applies: String?
    /// `grant` (kept in the profile) or `run` (shared by this session only).
    public var from: String?

    public var id: String { path }

    public init(path: String, mode: String, lastUsed: Date? = nil, openHandles: Int? = nil,
                guestPath: String? = nil, applies: String? = nil, from: String? = nil) {
        self.path = path
        self.mode = mode
        self.lastUsed = lastUsed
        self.openHandles = openHandles
        self.guestPath = guestPath
        self.applies = applies
        self.from = from
    }
}

public struct RuleJSON: Codable, Equatable, Sendable, Identifiable {
    public var host: String
    /// `allow` or `deny`.
    public var answer: String
    /// `always` or `session`.
    public var scope: String
    public var added: Date?
    public var lastUsed: Date?
    public var count: Int?
    /// True for a rule from the app profile's seed hosts.
    public var seeded: Bool?

    public var id: String { host + "|" + answer + "|" + scope }

    public init(host: String, answer: String, scope: String, added: Date? = nil,
                lastUsed: Date? = nil, count: Int? = nil, seeded: Bool? = nil) {
        self.host = host
        self.answer = answer
        self.scope = scope
        self.added = added
        self.lastUsed = lastUsed
        self.count = count
        self.seeded = seeded
    }
}

public struct ClipboardJSON: Codable, Equatable, Sendable {
    public var toApp: Bool
    public var fromApp: Bool

    public init(toApp: Bool, fromApp: Bool) {
        self.toApp = toApp
        self.fromApp = fromApp
    }
}

public struct DeviceJSON: Codable, Equatable, Sendable, Identifiable {
    public var name: String
    public var enabled: Bool
    /// Always `next_start`: devices attach when the app starts.
    public var applies: String?
    public var id: String { name }
}

public struct ChangeJSON: Codable, Equatable, Sendable, Identifiable {
    public var time: Date
    /// paddock's text: `Allow always api.example.com`.
    public var text: String
    /// `you`, `question_panel`, `profile`.
    public var source: String

    public var id: String { "\(time.timeIntervalSince1970)|\(text)|\(source)" }

    public init(time: Date, text: String, source: String) {
        self.time = time
        self.text = text
        self.source = source
    }
}

/// `paddock app status NAME --json`: the list fields plus the grants.
public struct AppStatusJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var app: AppJSON
    public var downloads: String?
    public var shares: [ShareJSON]?
    public var rules: [RuleJSON]?
    public var networkText: String?
    public var openConnections: Int?
    public var clipboard: ClipboardJSON?
    public var devices: [DeviceJSON]?
    public var changes: [ChangeJSON]?
    public var logPath: String?
}

// MARK: - Questions

public struct QuestionJSON: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var app: String
    public var host: String
    public var port: Int
    /// `Sample Notes.app → helper (Renderer)`.
    public var process: String?
    public var opened: Date?

    public init(id: String, app: String, host: String, port: Int, process: String? = nil, opened: Date? = nil) {
        self.id = id
        self.app = app
        self.host = host
        self.port = port
        self.process = process
        self.opened = opened
    }
}

public struct PausedJSON: Codable, Equatable, Sendable {
    public var app: String
    public var count: Int
    /// The current paddock notice card, absent in older snapshots.
    public var id: String? = nil
}

public struct ApprovalsJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var questions: [QuestionJSON]
    public var paused: PausedJSON?
    public var complete: Bool
    public var diagnostics: [String]

    private enum CodingKeys: String, CodingKey { case v, questions, paused, pending, complete, diagnostics }

    private struct Pending: Decodable {
        var id: String
        var capability: String
        var connect: Connection?
    }

    private struct Connection: Decodable {
        var profile: String?
        var host: String?
        var port: Int?
        var path: String?
        var askedAt: Double?
        var notice: Bool?
        var more: Int?
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        v = try values.decode(Int.self, forKey: .v)
        complete = try values.decodeIfPresent(Bool.self, forKey: .complete) ?? true
        diagnostics = try values.decodeIfPresent([String].self, forKey: .diagnostics) ?? []
        if let legacy = try values.decodeIfPresent([QuestionJSON].self, forKey: .questions) {
            questions = legacy
            paused = try values.decodeIfPresent(PausedJSON.self, forKey: .paused)
            return
        }
        let pending = try values.decode([Pending].self, forKey: .pending)
        questions = []
        paused = nil
        for item in pending where item.capability == "connect" {
            guard let c = item.connect else { continue }
            let app = c.profile ?? ""
            if c.notice == true {
                // The launcher currently presents one pause banner.
                if paused == nil { paused = PausedJSON(app: app, count: c.more ?? 0, id: item.id) }
            } else {
                questions.append(QuestionJSON(id: item.id, app: app, host: c.host ?? "", port: c.port ?? 0,
                                              process: c.path,
                                              opened: c.askedAt.map { Date(timeIntervalSince1970: $0) }))
            }
        }
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(v, forKey: .v)
        try values.encode(questions, forKey: .questions)
        try values.encodeIfPresent(paused, forKey: .paused)
        try values.encode(complete, forKey: .complete)
        try values.encode(diagnostics, forKey: .diagnostics)
    }
}

// MARK: - Doctor and add precheck

public struct DoctorJSON: Codable, Equatable, Sendable {
    public var v: Int
    /// `apple` or `intel`.
    public var chip: String
    public var chipName: String?
    public var macos: String
    public var macosOk: Bool
    public var freeBytes: Int64
    public var needBytes: Int64
    public var memoryBytes: Int64
    public var vmEntitlement: Bool?
    /// Names of the apps that hold the two VM slots.
    public var slots: [String]?
}

public struct PrecheckJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var ok: Bool
    public var name: String
    public var version: String?
    public var sizeBytes: Int64?
    public var appStore: Bool?
    /// The maker's site when paddock's profile knows it.
    public var website: String?
    /// `none`, `building`, `ready`.
    public var golden: String?
    public var needBytes: Int64?
    public var freeBytes: Int64?
    public var memoryOptionsGb: [Int]?
    public var message: String?
}

/// The answer of a command that changes something. `ok` false carries the
/// reason in one of the optional fields.
public struct ResultJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var ok: Bool
    public var error: String?
    /// A share or rule paddock refuses, with paddock's line.
    public var refused: String?
    /// A folder with a `.env` or key file: paddock's line in `message`.
    public var needsUnprotected: Bool?
    public var message: String?
    /// `live`, `next_start`, `reopen`.
    public var applies: String?
    public var share: ShareJSON?
    public var rule: RuleJSON?
    public var freedBytes: Int64?
    public var launchpadName: String?
    /// `app run` refusals: `two_vms`, `low_disk`, `app_store`, `no_golden`.
    public var code: String?
    public var running: [String]?
    public var needBytes: Int64?
    public var freeBytes: Int64?
    public var openHandles: Int?
    /// `macvm latest`: the newest macOS paddock can build.
    public var latest: String?
    public var current: String?

    public init(v: Int = 1, ok: Bool) {
        self.v = v
        self.ok = ok
    }
}
