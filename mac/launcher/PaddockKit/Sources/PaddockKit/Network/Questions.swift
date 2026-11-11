import Foundation

/// The seven answers of paddock's question panel (src/broker/approval.rs,
/// `ConnectAnswer`). The word is the flag of `paddock approvals answer`.
public enum PanelAnswer: String, CaseIterable, Equatable, Sendable {
    case deny
    case allowOnce = "allow-once"
    case allowSite = "allow-site"
    case allowSession = "allow-session"
    case denyAlways = "deny-always"
    case allowAlways = "allow-always"
    case allowEverything = "allow-everything"

    public var flag: String { "--" + rawValue }

    /// True for an answer that writes a rule which outlives this question.
    public var makesRule: Bool {
        switch self {
        case .deny, .allowOnce: return false
        default: return true
        }
    }
}

/// One answer button of a question, with its title and the spoken label.
public struct QuestionChoice: Identifiable, Equatable, Sendable {
    public var answer: PanelAnswer
    public var title: UIText
    public var label: UIText
    public var id: String { answer.rawValue }
}

/// The answers of a question in the order of paddock's panel
/// (src/mark/panels.rs): the row `Deny`, `Allow once`, `Allow <site>`, and
/// the `More…` menu `Allow this session`, `Always deny`, `Always allow`
/// (this host only) and `Allow everything for 10 minutes`.
public enum QuestionChoices {
    /// The site of the question, or nil for an IP address.
    public static func site(_ q: QuestionJSON) -> String? { NetSite.registrable(q.host) }

    /// What `Allow <site>` allows: the site, else the host.
    public static func target(_ q: QuestionJSON) -> String { site(q) ?? NetSite.normalized(q.host) }

    public static func title(_ q: QuestionJSON) -> UIText {
        UIText("%@ wants to connect to %@", q.app, q.host)
    }

    public static func detail(_ q: QuestionJSON) -> UIText {
        if let name = site(q) {
            return UIText("port %@ · %@ and all its subdomains", "\(q.port)", name)
        }
        return UIText("port %@ · first connection to this host", "\(q.port)")
    }

    public static func row(_ q: QuestionJSON) -> [QuestionChoice] {
        let allowed = target(q)
        let siteLabel = site(q) != nil ? UIText("Allow %@ and all its subdomains", allowed)
            : UIText("Always allow %@", allowed)
        return [
            QuestionChoice(answer: .deny, title: UIText("Deny"), label: UIText("Deny for %@", q.host)),
            QuestionChoice(answer: .allowOnce, title: UIText("Allow once"), label: UIText("Allow once for %@", q.host)),
            QuestionChoice(answer: .allowSite, title: UIText("Allow %@", allowed), label: siteLabel),
        ]
    }

    public static func menu(_ q: QuestionJSON) -> [QuestionChoice] {
        [
            QuestionChoice(answer: .allowSession, title: UIText("Allow this session"),
                           label: UIText("Allow this session for %@", q.host)),
            QuestionChoice(answer: .denyAlways, title: UIText("Always deny"),
                           label: UIText("Always deny for %@", q.host)),
            QuestionChoice(answer: .allowAlways, title: UIText("Always allow %@ only", q.host),
                           label: UIText("Always allow %@ only", q.host)),
            QuestionChoice(answer: .allowEverything, title: UIText("Allow everything for 10 minutes"),
                           label: UIText("Allow everything for 10 minutes")),
        ]
    }
}

extension LauncherStore {
    /// Answers a question with one of the panel's answers. paddock's panel
    /// closes too. Returns paddock's line when the answer did not arrive;
    /// the question then stays in the list.
    @discardableResult
    public func answer(_ question: QuestionJSON, panel answer: PanelAnswer) async -> String? {
        guard let service else { return UIText("paddock is not reachable").english }
        do {
            try await service.execute(NetworkArgs.answer(question.id, answer))
            questions.removeAll { $0.id == question.id }
            return nil
        } catch {
            return LauncherStore.line(of: error)
        }
    }
}
