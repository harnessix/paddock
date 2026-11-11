import Foundation

/// What the person typed into the `Host` field.
public enum HostInput: Equatable, Sendable {
    /// A host name: `api.example.com`.
    case host(String)
    /// Every name under a domain: the glob `*.example.com`.
    case wildcard(String)
    /// An IP address.
    case address(String)
    /// `*`: paddock allows that only in Terminal.
    case everyHost
    /// A wildcard over a public suffix, such as `*.co.uk` or `*.com`.
    case tooBroad(String)
    case empty
    case invalid

    /// The glob for paddock, or nil when the input cannot be a rule.
    public var glob: String? {
        switch self {
        case .host(let value), .wildcard(let value), .address(let value): return value
        default: return nil
        }
    }

    /// Reads the field. A pasted URL counts as its host, and its port is
    /// returned too: `https://api.example.com:8443/x` is `api.example.com`
    /// and 8443.
    public static func parse(_ text: String) -> (input: HostInput, port: Int?) {
        var rest = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if rest.isEmpty { return (.empty, nil) }
        if let scheme = rest.range(of: "://") { rest = String(rest[scheme.upperBound...]) }
        if let slash = rest.firstIndex(where: { "/?#".contains($0) }) { rest = String(rest[..<slash]) }
        if let at = rest.lastIndex(of: "@") { rest = String(rest[rest.index(after: at)...]) }
        var port: Int?
        if rest.hasPrefix("[") {
            // [::1]:443
            if let close = rest.firstIndex(of: "]") {
                let after = rest[rest.index(after: close)...]
                if after.hasPrefix(":") { port = Int(after.dropFirst()) }
                rest = String(rest[rest.index(after: rest.startIndex)..<close])
            }
        } else if rest.filter({ $0 == ":" }).count == 1, let colon = rest.firstIndex(of: ":") {
            port = Int(rest[rest.index(after: colon)...])
            if port == nil { return (.invalid, nil) }
            rest = String(rest[..<colon])
        }
        let name = NetSite.normalized(rest)
        if name.isEmpty { return (.invalid, port) }
        if name == "*" || name == "*.*" { return (.everyHost, port) }
        if NetSite.isAddress(name) { return (.address(name), port) }
        if name.hasPrefix("*.") {
            let base = String(name.dropFirst(2))
            guard NetSite.validName(base) else { return (.invalid, port) }
            // A wildcard must stay inside one site: `*.example.com` is fine,
            // `*.com` and `*.co.uk` would cover strangers.
            guard NetSite.registrable(base) != nil else { return (.tooBroad(name), port) }
            return (.wildcard(name), port)
        }
        guard NetSite.validName(name) else { return (.invalid, port) }
        return (.host(name), port)
    }
}

/// The port choice of the rule editor.
public enum PortChoice: String, CaseIterable, Equatable, Sendable, Identifiable {
    case https, http, any, other

    public var id: String { rawValue }

    public var title: UIText {
        switch self {
        case .https: return UIText("443 (HTTPS)")
        case .http: return UIText("80 (HTTP)")
        case .any: return UIText("Any port")
        case .other: return UIText("Other…")
        }
    }

    public static func of(_ port: RulePort) -> (PortChoice, String) {
        switch port {
        case .any: return (.any, "")
        case .number(443): return (.https, "")
        case .number(80): return (.http, "")
        case .number(let value): return (.other, "\(value)")
        }
    }
}

/// A rule after the checks of the editor: what goes to paddock.
public struct CheckedRule: Equatable, Sendable {
    public var host: String
    public var port: RulePort
    /// Also every subdomain: paddock writes `host` and `*.host`.
    public var site: Bool
    public var verdict: RuleAnswer
    public var scope: RuleScope
}

/// The fields of the rule editor, for Add and Edit.
public struct RuleDraft: Equatable, Sendable {
    public var host = ""
    public var site = false
    public var port: PortChoice = .https
    public var otherPort = ""
    public var verdict: RuleAnswer = .allow
    public var scope: RuleScope = .always

    public init() {}

    /// The fields of an existing row.
    public init(row: RuleRow) {
        host = row.host
        site = row.isSite
        (port, otherPort) = PortChoice.of(row.port)
        verdict = row.verdict == "deny" ? .deny : .allow
        scope = row.scope == "session" ? .session : .always
    }

    /// The site that `Include the whole site` adds, or nil when the host
    /// has none (an IP address, a wildcard, a bad name).
    public var siteSuggestion: String? {
        if case .host(let name) = HostInput.parse(host).input { return NetSite.registrable(name) }
        return nil
    }

    /// The rule to send, or the inline message under the field.
    /// `running` false refuses a rule that ends with the session.
    public func check(app: String, running: Bool) -> Result<CheckedRule, NetMessage> {
        let parsed = HostInput.parse(host)
        let glob: String
        switch parsed.input {
        case .empty, .invalid:
            return .failure(NetMessage(UIText("Enter a host name, for example api.example.com.")))
        case .everyHost:
            return .failure(NetMessage(UIText("A rule for every host turns the firewall off for the app. paddock allows it only in Terminal:"),
                                       command: ShellCommands.allHostsRule(app: app)))
        case .tooBroad(let value):
            return .failure(NetMessage(UIText("%@ covers sites of many owners. Enter a name under it, for example *.example.com.", value)))
        case .host(let value), .wildcard(let value), .address(let value):
            glob = value
        }
        var rulePort: RulePort
        switch port {
        case .https: rulePort = .number(443)
        case .http: rulePort = .number(80)
        case .any: rulePort = .any
        case .other:
            guard let value = Int(otherPort.trimmingCharacters(in: .whitespaces)), (1...65535).contains(value) else {
                return .failure(NetMessage(UIText("Enter a port from 1 to 65535.")))
            }
            rulePort = .number(value)
        }
        if let pasted = parsed.port, port == .https, pasted != 443 { rulePort = .number(pasted) }
        if scope == .session, !running {
            return .failure(NetMessage(UIText("The app is not running. Choose Always, or start the app first.")))
        }
        var sendHost = glob
        let wantsSite = site && siteSuggestion != nil
        if wantsSite, let suggestion = siteSuggestion { sendHost = suggestion }
        return .success(CheckedRule(host: sendHost, port: rulePort, site: wantsSite, verdict: verdict, scope: scope))
    }
}

/// An inline message, with a command to copy when the fix is in Terminal.
public struct NetMessage: Error, Equatable, Sendable {
    public var text: UIText
    public var command: String?

    public init(_ text: UIText, command: String? = nil) {
        self.text = text
        self.command = command
    }
}

/// Who made a rule.
public enum RuleMaker: Equatable, Sendable {
    case you, questionPanel, profile, approver

    public static func of(_ rule: NetRuleJSON) -> RuleMaker {
        switch rule.source {
        case "question_panel"?: return .questionPanel
        case "approver"?: return .approver
        case "profile"?: return .profile
        case "you"?: return .you
        default: break
        }
        switch rule.by {
        case "profile"?: return .profile
        case "approver"?: return .approver
        default: return .you
        }
    }

    public var title: UIText {
        switch self {
        case .you: return UIText("You")
        case .questionPanel: return UIText("Question panel")
        case .profile: return UIText("App profile")
        case .approver: return UIText("Approver")
        }
    }
}

/// One row of the rules table: one rule, or the pair `example.com` and
/// `*.example.com` that `Allow site` writes, shown as one site.
public struct RuleRow: Identifiable, Equatable, Sendable {
    public var rules: [NetRuleJSON]
    /// The host of the first rule; the site for a pair.
    public var host: String
    public var isSite: Bool

    /// `rules` must not be empty.
    public init(rules: [NetRuleJSON], host: String, isSite: Bool) {
        precondition(!rules.isEmpty, "a rule row needs a rule")
        self.rules = rules
        self.host = host
        self.isSite = isSite
    }

    public var id: String { rules.map(\.id).joined(separator: "+") }
    public var ids: [String] { rules.map(\.id) }
    var first: NetRuleJSON { rules[0] }
    public var port: RulePort { first.port }
    public var verdict: String { first.verdict }
    public var scope: String { first.scope }
    public var maker: RuleMaker { RuleMaker.of(first) }
    public var isProfile: Bool { maker == .profile }
    public var made: Date? { first.made }
    public var expires: Date? { rules.compactMap(\.expires).min() }
    public var lastUsed: Date? { rules.compactMap(\.lastUsed).max() }
    public var count: Int { rules.reduce(0) { $0 + ($1.count ?? 0) } }

    /// The rows of a rule list: expired rules dropped, site pairs joined,
    /// the newest first, the rules of the profile last.
    public static func rows(_ rules: [NetRuleJSON], now: Date) -> [RuleRow] {
        let live = rules.filter { rule in rule.expires.map { $0 > now } ?? true }
        var used = Set<String>()
        var rows: [RuleRow] = []
        for rule in live where !used.contains(rule.id) {
            used.insert(rule.id)
            if !rule.host.hasPrefix("*."),
               let partner = live.first(where: { other in
                   !used.contains(other.id) && other.host == "*." + rule.host && other.port == rule.port
                       && other.verdict == rule.verdict && other.scope == rule.scope && other.by == rule.by
               }) {
                used.insert(partner.id)
                rows.append(RuleRow(rules: [rule, partner], host: rule.host, isSite: true))
            } else if rule.host.hasPrefix("*."),
                      let partner = live.first(where: { other in
                          !used.contains(other.id) && "*." + other.host == rule.host && other.port == rule.port
                              && other.verdict == rule.verdict && other.scope == rule.scope && other.by == rule.by
                      }) {
                used.insert(partner.id)
                rows.append(RuleRow(rules: [partner, rule], host: partner.host, isSite: true))
            } else {
                rows.append(RuleRow(rules: [rule], host: rule.host, isSite: false))
            }
        }
        return rows.enumerated().sorted { a, b in
            if a.element.isProfile != b.element.isProfile { return !a.element.isProfile }
            let ta = a.element.made ?? .distantPast
            let tb = b.element.made ?? .distantPast
            if ta != tb { return ta > tb }
            return a.offset < b.offset
        }.map(\.element)
    }
}

/// The texts of the rules table.
public enum NetRuleCopy {
    public static func host(_ row: RuleRow) -> UIText {
        row.isSite ? UIText("%@ and its subdomains", row.host) : UIText(key: "%@", args: [row.host])
    }

    public static func port(_ port: RulePort) -> UIText {
        switch port {
        case .any: return UIText("Any port")
        case .number(let value): return UIText(key: "%@", args: ["\(value)"])
        }
    }

    public static func verdict(_ verdict: String) -> UIText {
        verdict == "deny" ? UIText("Deny") : UIText("Allow")
    }

    public static func scope(_ scope: String) -> UIText {
        scope == "session" ? UIText("Until the app stops") : UIText("Always")
    }

    /// `9 min left` for a temporary rule, nil for one without an end.
    public static func timeLeft(_ row: RuleRow, now: Date) -> UIText? {
        guard let end = row.expires else { return nil }
        return Format.timeLeft(seconds: max(0, end.timeIntervalSince(now)))
    }

    /// The line of the Changes list for a rule the person wrote.
    public static func change(_ rule: CheckedRule) -> String {
        let what = rule.site ? rule.host + " and its subdomains" : rule.host
        switch (rule.verdict, rule.scope) {
        case (.allow, .always): return "Allow always \(what)"
        case (.allow, .session): return "Allow this session \(what)"
        case (.deny, .always): return "Deny always \(what)"
        case (.deny, .session): return "Deny this session \(what)"
        }
    }

    /// The text of the confirmation before Remove.
    public static func removeTitle(_ rows: [RuleRow]) -> UIText {
        if rows.count == 1, let row = rows.first {
            return row.isSite ? UIText("Remove the rule for %@ and its subdomains?", row.host)
                : UIText("Remove the rule for %@?", row.host)
        }
        return UIText("Remove %@ rules?", "\(rows.count)")
    }
}
