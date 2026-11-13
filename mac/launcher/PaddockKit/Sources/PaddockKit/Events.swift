import Foundation

/// One line of `paddock events --follow --json`.
///
/// Each line is a JSON object with `v` and `kind`. The launcher reads the
/// kinds `session`, `question`, `grant`, `setup`, `golden`, `disk` and
/// `notice`, and ignores any other kind, so a newer paddock can add kinds.
public enum PaddockEvent: Equatable, Sendable {
    case session(SessionEvent)
    case question(QuestionEvent)
    case grant(GrantEvent)
    case setup(SetupJSON)
    case golden(op: String)
    case disk(freeBytes: Int64)
    /// `started` (the first line) or `lost` (the follower missed events).
    case stream(state: String)
    /// A desktop notification of paddock, which the launcher shows.
    case notice(NoticeEvent)
    case unknown(kind: String)

    /// Decodes one line. Returns nil for a line that is not a JSON object
    /// with a `kind`.
    public static func decode(line: String) -> PaddockEvent? {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return nil }
        let decoder = PaddockJSON.decoder
        guard let head = try? decoder.decode(EventHead.self, from: data) else { return nil }
        switch head.kind {
        case "session":
            return (try? decoder.decode(SessionEvent.self, from: data)).map(PaddockEvent.session)
        case "question":
            return (try? decoder.decode(QuestionEvent.self, from: data)).map(PaddockEvent.question)
        case "grant":
            return (try? decoder.decode(GrantEvent.self, from: data)).map(PaddockEvent.grant)
        case "setup":
            return LibraryDecode.setupLine(data).map(PaddockEvent.setup)
        case "golden":
            // paddock writes `state`; the older fixtures `op`.
            return (try? decoder.decode(GoldenEvent.self, from: data)).flatMap { e in
                (e.op ?? e.state).map { PaddockEvent.golden(op: $0) }
            }
        case "disk":
            return (try? decoder.decode(DiskEvent.self, from: data)).flatMap { e in
                (e.freeBytes ?? e.free).map { PaddockEvent.disk(freeBytes: $0) }
            }
        case "stream":
            return (try? decoder.decode(StreamEvent.self, from: data)).map { .stream(state: $0.state) }
        case "notice":
            return (try? decoder.decode(NoticeEvent.self, from: data)).map(PaddockEvent.notice)
        default:
            return .unknown(kind: head.kind)
        }
    }
}

struct EventHead: Decodable {
    var v: Int?
    var kind: String
}

struct GoldenEvent: Decodable {
    /// `ready` or `removed`.
    var op: String?
    var state: String?
}

struct DiskEvent: Decodable {
    var freeBytes: Int64?
    /// paddock's key.
    var free: Int64?
}

struct StreamEvent: Decodable {
    var state: String
}

/// A notification that paddock leaves to the launcher. paddock does not
/// post it itself while the launcher follows with `--notify`.
public struct NoticeEvent: Codable, Equatable, Sendable {
    /// What it is about: `approval`, `question`, `frozen`, `storage`, `info`.
    public var what: String
    public var title: String
    public var text: String
    /// paddock asks for attention now.
    public var critical: Bool?
    /// What answers it, for example the request id of
    /// `paddock approvals answer ID`.
    public var action: String?

    public init(what: String, title: String, text: String, critical: Bool? = nil, action: String? = nil) {
        self.what = what
        self.title = title
        self.text = text
        self.critical = critical
        self.action = action
    }
}

public struct SessionEvent: Codable, Equatable, Sendable {
    public var app: String
    /// The app states of `AppJSON.state`, or `agent` for a guest agent
    /// change, or `removed` and `added` for the profile list.
    public var state: String
    public var phase: String?
    public var since: Date?
    public var windows: Int?
    public var reason: String?
    public var blockCode: String?
    public var crash: String?
    public var connected: Bool?
    public var nextStart: String?
    /// paddock's home folder of the instance: `default` or `instance-N`.
    public var home: String?
    /// paddock's line for the change: `quit with save`, `opened`.
    public var text: String?
    /// The time of the event.
    public var t: Date?
}

/// paddock's session words to the launcher's states.
public enum SessionWords {
    /// `stopped` after `quit with save` is `saved`: the next start is warm.
    public static func state(_ e: SessionEvent) -> String {
        if e.state == "stopped", e.text == "quit with save" { return "saved" }
        return e.state
    }
}

public struct QuestionEvent: Codable, Equatable, Sendable {
    /// Current paddock writes `state`; older fixtures used `op`.
    public var state: String?
    public var op: String?
    public var id: String?
    public var app: String?
    public var host: String?
    public var port: Int?
    public var process: String?
    public var opened: Date?
    /// For `answered`: `deny`, `allow-once`, `allow-session`, `allow-always`, `deny-always`.
    public var answer: String?
    /// For `answered`: `panel`, `launcher`, `cli`.
    public var source: String?
    /// Current paddock writes `by` and `open`.
    public var by: String?
    public var open: Int?
    /// For `paused`: the questions in the last minute.
    public var count: Int?
}

public struct GrantEvent: Codable, Equatable, Sendable {
    public var app: String
    /// `folder`, `rule`, `network`, `clipboard`, `device`, `downloads`.
    public var grant: String
    /// `add`, `remove`, `change`.
    public var op: String
    /// paddock's text for the Changes list.
    public var detail: String
    /// `live`, `next_start`, `reopen`.
    public var applies: String?
    /// `you`, `question_panel`, `profile`.
    public var source: String
    public var time: Date?

    private enum CodingKeys: String, CodingKey {
        case app, grant, op, detail, applies, source, time
        case what, action, t
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        app = try values.decode(String.self, forKey: .app)
        grant = try values.decodeIfPresent(String.self, forKey: .grant)
            ?? values.decode(String.self, forKey: .what)
        op = try values.decodeIfPresent(String.self, forKey: .op)
            ?? values.decode(String.self, forKey: .action)
        detail = try values.decode(String.self, forKey: .detail)
        applies = try values.decodeIfPresent(String.self, forKey: .applies)
        source = try values.decode(String.self, forKey: .source)
        time = try values.decodeIfPresent(Date.self, forKey: .time)
            ?? values.decodeIfPresent(Date.self, forKey: .t)
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(app, forKey: .app)
        try values.encode(grant, forKey: .grant)
        try values.encode(op, forKey: .op)
        try values.encode(detail, forKey: .detail)
        try values.encodeIfPresent(applies, forKey: .applies)
        try values.encode(source, forKey: .source)
        try values.encodeIfPresent(time, forKey: .time)
    }
}
