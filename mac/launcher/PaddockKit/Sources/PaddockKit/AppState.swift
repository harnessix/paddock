import Foundation

/// The phases of an app start, in the order paddock prints them.
public enum StartPhase: String, Equatable, Sendable, CaseIterable {
    case cloning
    case booting
    case copyingApp = "copying_app"
    case opening
    case resuming

    public init(json: String?) {
        let key = (json ?? "").replacingOccurrences(of: " ", with: "_")
        self = StartPhase(rawValue: key) ?? .cloning
    }

    public var rowText: UIText {
        switch self {
        case .cloning: return UIText("Starting… cloning")
        case .booting: return UIText("Starting… booting")
        case .copyingApp: return UIText("Starting… copying app")
        case .opening: return UIText("Starting… opening")
        case .resuming: return UIText("Starting… resuming")
        }
    }
}

/// The app states the launcher shows. Each comes from paddock. The
/// launcher never invents one.
public enum AppState: Equatable, Sendable {
    case stopped
    case saved(since: Date?)
    case starting(StartPhase)
    case running(since: Date?, windows: Int)
    /// paddock saved the session to free a VM slot. `app run` resumes it.
    case suspended(reason: String?)
    case stopping
    case error(reason: String)
    case blocked(reason: String, code: String?)

    public init(json: AppJSON) {
        self.init(state: json.state, phase: json.phase, since: json.since, windows: json.windows,
                  reason: json.reason ?? json.suspendReason, blockCode: json.blockCode)
    }

    public init(state: String, phase: String?, since: Date?, windows: Int?, reason: String?, blockCode: String?) {
        switch state {
        case "saved": self = .saved(since: since)
        case "starting": self = .starting(StartPhase(json: phase))
        case "running": self = .running(since: since, windows: windows ?? 0)
        case "suspended": self = .suspended(reason: reason)
        case "stopping": self = .stopping
        case "error": self = .error(reason: reason ?? "")
        case "blocked": self = .blocked(reason: reason ?? "", code: blockCode)
        default: self = .stopped
        }
    }

    public var isRunning: Bool {
        if case .running = self { return true }
        return false
    }

    /// Running or on the way there or back: the VM exists.
    public var isActive: Bool {
        switch self {
        case .running, .starting, .stopping: return true
        default: return false
        }
    }

    public var canStart: Bool {
        switch self {
        case .stopped, .saved, .error: return true
        default: return false
        }
    }

    /// Suspended: `app run` resumes from the saved state.
    public var canResume: Bool {
        if case .suspended = self { return true }
        return false
    }

    /// A suspended session stops at once: its state is on disk already.
    public var canStop: Bool {
        switch self {
        case .running, .starting, .suspended: return true
        default: return false
        }
    }

    /// The session holds a VM slot or a saved session: data must not go.
    public var holdsSession: Bool { isActive || canResume }
}

/// The tint of a status symbol. Only system colors, only in symbols.
public enum SymbolTint: String, Equatable, Sendable {
    case secondary, green, orange, red
}

/// How a sidebar row shows an app state: symbol, tint, the status text and
/// the VoiceOver label. A progress row shows a small spinner in place of
/// the symbol.
public struct StatusPresentation: Equatable, Sendable {
    public var symbol: String?
    public var tint: SymbolTint
    public var text: UIText
    public var accessibilityLabel: UIText
    public var showsProgress: Bool

    /// The one state word of the status pill: `Running`, `Ready`, `Error`.
    /// The row text under the name keeps the detail.
    public var word: UIText {
        switch kind {
        case .stopped: return UIText("Stopped")
        case .ready: return UIText("Ready")
        case .starting: return UIText("Starting")
        case .running: return UIText("Running")
        case .suspended: return UIText("Suspended")
        case .stopping: return UIText("Stopping")
        case .error: return UIText("Error")
        case .blocked: return UIText("Blocked")
        }
    }

    /// The state without its values, for the word and the motion rules.
    public enum Kind: String, Equatable, Sendable {
        case stopped, ready, starting, running, suspended, stopping, error, blocked
    }

    public var kind: Kind

    /// True while the app starts: the only state whose symbol pulses.
    public var pulses: Bool { kind == .starting }

    init(symbol: String?, tint: SymbolTint, text: UIText, accessibilityLabel: UIText, showsProgress: Bool,
         kind: Kind) {
        self.symbol = symbol
        self.tint = tint
        self.text = text
        self.accessibilityLabel = accessibilityLabel
        self.showsProgress = showsProgress
        self.kind = kind
    }

    public static func of(_ state: AppState, now: Date) -> StatusPresentation {
        switch state {
        case .stopped:
            return .init(symbol: "stop.circle", tint: .secondary, text: UIText("Stopped"),
                         accessibilityLabel: UIText("Stopped"), showsProgress: false, kind: .stopped)
        case .saved(let since):
            let text = since.map { UIText("Ready · saved %@", Format.age(since: $0, now: now).english) }
                ?? UIText("Ready")
            return .init(symbol: "pause.circle", tint: .secondary, text: text,
                         accessibilityLabel: UIText("Ready"), showsProgress: false, kind: .ready)
        case .starting(let phase):
            return .init(symbol: nil, tint: .secondary, text: phase.rowText,
                         accessibilityLabel: UIText("Starting"), showsProgress: true, kind: .starting)
        case .running(_, let windows):
            let text = windows == 1 ? UIText("Running · 1 window") : UIText("Running · %@ windows", "\(windows)")
            return .init(symbol: "play.circle.fill", tint: .green, text: text,
                         accessibilityLabel: UIText("Running"), showsProgress: false, kind: .running)
        case .suspended:
            return .init(symbol: "pause.circle.fill", tint: .secondary, text: UIText("Suspended · resumes in about 4 s"),
                         accessibilityLabel: UIText("Suspended"), showsProgress: false, kind: .suspended)
        case .stopping:
            return .init(symbol: nil, tint: .secondary, text: UIText("Stopping…"),
                         accessibilityLabel: UIText("Stopping"), showsProgress: true, kind: .stopping)
        case .error(let reason):
            return .init(symbol: "exclamationmark.triangle.fill", tint: .orange, text: UIText(key: "%@", args: [reason]),
                         accessibilityLabel: UIText("Error: %@", reason), showsProgress: false, kind: .error)
        case .blocked(let reason, _):
            return .init(symbol: "nosign", tint: .red, text: UIText(key: "%@", args: [reason]),
                         accessibilityLabel: UIText("Blocked: %@", reason), showsProgress: false, kind: .blocked)
        }
    }
}

/// The app as the launcher holds it: the list JSON plus the parsed state.
public struct AppItem: Equatable, Sendable, Identifiable {
    public var json: AppJSON
    public var state: AppState

    public init(json: AppJSON) {
        self.json = json
        self.state = AppState(json: json)
    }

    public var id: String { key }
    public var name: String { json.name }
}
