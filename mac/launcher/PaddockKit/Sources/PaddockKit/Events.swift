import Foundation

/// One line of `paddock events --follow --json`.
///
/// Each line is a JSON object with `v` and `kind`. The launcher reads the
/// kinds `session`, `question`, `grant`, `setup`, `golden` and `disk`, and
/// ignores any other kind, so a newer paddock can add kinds.
public enum PaddockEvent: Equatable, Sendable {
    case session(SessionEvent)
    case question(QuestionEvent)
    case grant(GrantEvent)
    case setup(SetupJSON)
    case golden(op: String)
    case disk(freeBytes: Int64)
    /// `started` (the first line) or `lost` (the follower missed events).
    case stream(state: String)
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
    /// `open`, `answered`, `paused`, `resumed`.
    public var op: String
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
}
