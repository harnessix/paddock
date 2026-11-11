import Foundation

/// The argument lists of the network and privacy calls. Unsupported
/// commands make the views show `Update paddock`.
public enum NetworkArgs {
    public static func rulesList(_ app: String) -> [String] { ["app", "rules", app, "list", "--json"] }

    public static func ruleAdd(_ app: String, _ rule: CheckedRule) -> [String] {
        var args = ["app", "rules", app, "add", rule.host, "--port", rule.port.argument, "--" + rule.verdict.rawValue]
        if rule.scope == .session { args.append("--session") }
        if rule.site { args.append("--site") }
        return args + ["--json"]
    }

    /// Changes a rule in place. For a site pair, `id` is the rule of the
    /// site itself; paddock changes its `*.` partner with it.
    public static func ruleChange(_ app: String, id: String, _ rule: CheckedRule) -> [String] {
        var args = ["app", "rules", app, "change", id, "--host", rule.host, "--port", rule.port.argument,
                    "--" + rule.verdict.rawValue, "--" + rule.scope.rawValue]
        if rule.site { args.append("--site") }
        return args + ["--json"]
    }

    public static func ruleRemove(_ app: String, ids: [String]) -> [String] {
        ["app", "rules", app, "remove"] + ids + ["--json"]
    }

    public static func connections(_ app: String) -> [String] {
        ["app", "connections", app, "--limit", "200", "--json"]
    }

    public static func answer(_ id: String, _ answer: PanelAnswer) -> [String] {
        ["approvals", "answer", id, answer.flag]
    }

    public static func clipboard(_ app: String) -> [String] { ["app", "clipboard", app, "--json"] }

    /// `session` true changes only the running session; the profile stays.
    public static func clipboard(_ app: String, _ value: ClipboardJSON, session: Bool) -> [String] {
        var args = ["app", "clipboard", app, "--to-app", value.toApp ? "on" : "off",
                    "--from-app", value.fromApp ? "on" : "off"]
        if session { args.append("--session") }
        return args + ["--json"]
    }

    /// Keeps a folder that only this session shares for later starts.
    public static func shareKeep(_ app: String, path: String) -> [String] {
        ["app", "share", app, "keep", path, "--json"]
    }
}
