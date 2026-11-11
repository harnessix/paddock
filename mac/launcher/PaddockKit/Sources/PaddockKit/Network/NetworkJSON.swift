import Foundation

// The JSON shapes of the network and privacy part of the launcher. They
// follow paddock's firewall rule (src/macvm/firewall.rs) and connection log
// (src/macvm/connlog.rs). The commands that print them are in
// `NetworkArgs`. Unknown fields are ignored and every new field is
// optional, so an older or newer paddock still decodes.

/// The port of a rule: one port, or every port (`*` in JSON).
public enum RulePort: Equatable, Hashable, Sendable, Codable {
    case any
    case number(Int)

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let value = try? container.decode(Int.self) {
            self = .number(value)
            return
        }
        let word = try container.decode(String.self)
        if word == "*" {
            self = .any
        } else if let value = Int(word) {
            self = .number(value)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "port")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .any: try container.encode("*")
        case .number(let value): try container.encode(value)
        }
    }

    /// The word of the `--port` flag.
    public var argument: String {
        switch self {
        case .any: return "*"
        case .number(let value): return "\(value)"
        }
    }
}

/// One firewall rule of an app profile, as `paddock app rules NAME list
/// --json` prints it.
public struct NetRuleJSON: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    /// The bundle id of the process, or `*` for any process of the app.
    public var app: String?
    public var team: String?
    /// `*`, `*.example.com`, `api.example.com` or an IP address.
    public var host: String
    public var port: RulePort
    /// `allow` or `deny`.
    public var verdict: String
    /// `always` or `session`.
    public var scope: String
    public var made: Date?
    /// When a temporary rule stops applying.
    public var expires: Date?
    /// `user`, `profile` or `approver`.
    public var by: String?
    /// Where a rule of the person came from: `you`, `question_panel`.
    public var source: String?
    public var lastUsed: Date?
    public var count: Int?

    public init(id: String, host: String, port: RulePort = .number(443), verdict: String = "allow",
                scope: String = "always", made: Date? = nil, expires: Date? = nil, by: String? = "user",
                source: String? = nil, lastUsed: Date? = nil, count: Int? = nil) {
        self.id = id
        self.host = host
        self.port = port
        self.verdict = verdict
        self.scope = scope
        self.made = made
        self.expires = expires
        self.by = by
        self.source = source
        self.lastUsed = lastUsed
        self.count = count
    }
}

/// `paddock app rules NAME list --json`.
public struct NetRulesJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var app: String?
    public var running: Bool?
    public var rules: [NetRuleJSON]
}

/// The answer of `rules add`, `rules change` and `rules remove`. `ok`
/// false carries paddock's line in `refused` or `error`.
public struct NetResultJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var ok: Bool
    /// The rules that the command wrote: one, or two for a site.
    public var rules: [NetRuleJSON]?
    /// The ids that the command removed.
    public var removed: [String]?
    public var refused: String?
    public var error: String?
    public var message: String?
    /// `live` or `next_start`.
    public var applies: String?

    public init(v: Int = 1, ok: Bool) {
        self.v = v
        self.ok = ok
    }

    /// paddock's line for a refusal, or nil.
    public var line: String? { refused ?? error ?? message }
}

/// One line of the connection log of a session: the same app, host, port
/// and verdict within a minute share a line and a count.
public struct ConnectionJSON: Codable, Equatable, Sendable, Identifiable {
    public var time: Date
    public var last: Date?
    public var host: String
    public var port: Int
    /// `allow`, `deny` or `ask`.
    public var verdict: String
    /// The rule id that decided, or `ask`, `once`, `timeout`, `mode`, `egress`.
    public var rule: String?
    /// `Sample Notes.app → helper (Renderer)`.
    public var process: String?
    public var count: Int?

    public var id: String { "\(host):\(port)|\(verdict)|\(time.timeIntervalSince1970)" }

    public init(time: Date, host: String, port: Int, verdict: String, rule: String? = nil,
                process: String? = nil, count: Int? = nil, last: Date? = nil) {
        self.time = time
        self.host = host
        self.port = port
        self.verdict = verdict
        self.rule = rule
        self.process = process
        self.count = count
        self.last = last
    }

    public var isDenied: Bool { verdict == "deny" }
    /// The latest moment of the line.
    public var seen: Date { last ?? time }
}

/// `paddock app connections NAME --json`.
public struct ConnectionsJSON: Codable, Equatable, Sendable {
    public var v: Int
    public var app: String?
    public var running: Bool?
    /// True when the lines are from a session that already ended.
    public var lastSession: Bool?
    public var connections: [ConnectionJSON]
}

/// `paddock app clipboard NAME --json`: the directions saved in the
/// profile and, while the app runs, the ones of the session. An older
/// paddock prints only `to_app` and `from_app`: those are the profile.
public struct ClipboardStateJSON: Decodable, Equatable, Sendable {
    public var profile: ClipboardJSON
    public var session: ClipboardJSON?
    public var running: Bool

    enum CodingKeys: String, CodingKey {
        case profile, session, running, toApp, fromApp
    }

    public init(profile: ClipboardJSON, session: ClipboardJSON?, running: Bool) {
        self.profile = profile
        self.session = session
        self.running = running
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        if let profile = try container.decodeIfPresent(ClipboardJSON.self, forKey: .profile) {
            self.profile = profile
        } else {
            profile = ClipboardJSON(toApp: try container.decode(Bool.self, forKey: .toApp),
                                    fromApp: try container.decode(Bool.self, forKey: .fromApp))
        }
        session = try container.decodeIfPresent(ClipboardJSON.self, forKey: .session)
        running = try container.decodeIfPresent(Bool.self, forKey: .running) ?? (session != nil)
    }
}
