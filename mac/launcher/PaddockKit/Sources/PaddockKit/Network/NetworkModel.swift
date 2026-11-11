import Foundation
import Observation

/// Where a list of the network part stands. Every state has a screen.
public enum NetLoad: Equatable, Sendable {
    case idle
    case loading
    case loaded
    /// paddock answered with an error: its line, shown with `Try Again`.
    case failed(String)
    /// This paddock lacks the command: the view shows the upgrade command.
    case needsNewerPaddock
    /// No usable paddock: missing, too old for the launcher, or not found.
    case offline

    /// The state for a failed call.
    public static func of(_ error: Error) -> NetLoad {
        switch error {
        case CommandError.notFound:
            return .offline
        case CommandError.failed(let status, let line):
            // clap exits with 2 and names the word it does not know.
            let unknown = ["unrecognized subcommand", "unexpected argument", "invalid value", "unrecognized option"]
            if status == 2, unknown.contains(where: { line.contains($0) }) { return .needsNewerPaddock }
            return .failed(line.isEmpty ? UIText("paddock stopped with status %@.", "\(status)").english : line)
        default:
            return .failed(LauncherStore.line(of: error))
        }
    }

    /// paddock's own line for a failure: the `error:` line of an argument
    /// error (clap ends it with a usage hint), else the last line.
    public static func errorLine(_ stderr: String) -> String {
        let lines = stderr.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
        if let error = lines.first(where: { $0.hasPrefix("error:") }) { return error }
        return CLIPaddockService.lastLine(stderr)
    }

    public var isFailure: Bool {
        switch self {
        case .failed, .needsNewerPaddock, .offline: return true
        default: return false
        }
    }
}

extension PaddockService {
    /// Runs a command that prints JSON, also when it exits non-zero with a
    /// refusal: the JSON wins over the exit status, as in `perform`.
    func decoded<T: Decodable>(_ args: [String], as type: T.Type) async throws -> T {
        if let cli = self as? CLIPaddockService {
            let out = try await cli.runner.run(args)
            if let value = try? PaddockJSON.decoder.decode(T.self, from: out.stdout) { return value }
            if out.status != 0 { throw CommandError.failed(status: out.status, line: NetLoad.errorLine(out.stderr)) }
            throw CommandError.badJSON(output: String(out.stdoutText.prefix(200)))
        }
        let text = try await self.text(args)
        do {
            return try PaddockJSON.decoder.decode(T.self, from: Data(text.utf8))
        } catch {
            throw CommandError.badJSON(output: String(text.prefix(200)))
        }
    }
}

/// The network and privacy model of one app: its rules, its recent
/// connections and its clipboard. The Grants tab owns one per app.
@MainActor
@Observable
public final class NetworkModel {
    public let name: String
    public var rules: [NetRuleJSON] = []
    public var rulesState: NetLoad = .idle
    public var connections: [ConnectionJSON] = []
    public var connectionsState: NetLoad = .idle
    /// The connections are from a session that already ended.
    public var lastSession = false
    public var clipboard: ClipboardStateJSON?
    public var clipboardState: NetLoad = .idle
    /// Rows of the connections list that got a rule: id to the row text.
    public var allowed: [String: UIText] = [:]
    /// paddock's line after a failed change, shown above the list.
    public var problem: NetMessage?

    @ObservationIgnored weak var store: LauncherStore?
    /// The time between two connection loads while the list is on screen.
    public static let connectionsEvery: Double = 10

    public init(name: String, store: LauncherStore) {
        self.name = name
        self.store = store
    }

    var service: PaddockService? {
        guard let store, store.binary.isUsable else { return nil }
        return store.service
    }

    var now: Date { store?.clock.now() ?? Date() }

    public var isRunning: Bool { store?.app(named: name)?.state.isActive ?? false }

    public func rows(now: Date) -> [RuleRow] { RuleRow.rows(rules, now: now) }

    /// Allowed once per key every 5 s, or always with `force`.
    func allow(_ key: String, force: Bool) -> Bool {
        guard let store else { return false }
        return store.gate.allow(key + " " + name, now: store.clock.now()) || force
    }

    // MARK: Load

    public func loadRules(force: Bool = false) async {
        guard let service else {
            rulesState = .offline
            return
        }
        guard allow("app rules list", force: force) else { return }
        if rules.isEmpty, !rulesState.isFailure { rulesState = .loading }
        do {
            let list = try await service.decoded(NetworkArgs.rulesList(name), as: NetRulesJSON.self)
            rules = list.rules
            rulesState = .loaded
        } catch {
            rulesState = NetLoad.of(error)
        }
    }

    public func loadConnections(force: Bool = false) async {
        guard let service else {
            connectionsState = .offline
            return
        }
        guard allow("app connections", force: force) else { return }
        if connections.isEmpty, !connectionsState.isFailure { connectionsState = .loading }
        do {
            let list = try await service.decoded(NetworkArgs.connections(name), as: ConnectionsJSON.self)
            connections = list.connections.sorted { $0.seen > $1.seen }
            lastSession = list.lastSession ?? false
            connectionsState = .loaded
        } catch {
            connectionsState = NetLoad.of(error)
        }
    }

    public func loadClipboard(force: Bool = false) async {
        guard let service else {
            clipboardState = .offline
            return
        }
        guard allow("app clipboard", force: force) else { return }
        if clipboard == nil { clipboardState = .loading }
        do {
            clipboard = try await service.decoded(NetworkArgs.clipboard(name), as: ClipboardStateJSON.self)
            clipboardState = .loaded
        } catch {
            clipboardState = NetLoad.of(error)
        }
    }

    /// Follows the connections while the list is on screen. The caller
    /// runs it in a task that ends with the view.
    public func followConnections() async {
        await loadConnections(force: true)
        while !Task.isCancelled {
            guard let clock = store?.clock else { return }
            try? await clock.sleep(seconds: NetworkModel.connectionsEvery)
            if Task.isCancelled { return }
            await loadConnections()
        }
    }

    // MARK: Rules

    /// `Add` or `Save` of the rule editor. Returns the inline message, or
    /// nil when paddock took the rule.
    public func save(_ draft: RuleDraft, editing: RuleRow? = nil) async -> NetMessage? {
        let rule: CheckedRule
        switch draft.check(app: name, running: isRunning) {
        case .failure(let message): return message
        case .success(let checked): rule = checked
        }
        guard let service else { return NetMessage(UIText("paddock is not reachable")) }
        let args = editing.map { NetworkArgs.ruleChange(name, id: $0.ids[0], rule) } ?? NetworkArgs.ruleAdd(name, rule)
        do {
            let result = try await service.decoded(args, as: NetResultJSON.self)
            guard result.ok else {
                return NetMessage(UIText(key: "%@", args: [result.line ?? UIText("paddock did not take the rule.").english]))
            }
            let old = Set(editing?.ids ?? []).union(result.removed ?? [])
            rules.removeAll { old.contains($0.id) }
            if let written = result.rules, !written.isEmpty {
                rules.removeAll { rule in written.contains { $0.id == rule.id } }
                rules.append(contentsOf: written)
            } else {
                await loadRules(force: true)
            }
            store?.detail(for: name).recordMine(NetRuleCopy.change(rule))
            return nil
        } catch {
            switch NetLoad.of(error) {
            case .needsNewerPaddock:
                return NetMessage(UIText("This paddock cannot change rules from the app yet. Update paddock:"),
                                  command: ShellCommands.brewUpgrade)
            case .failed(let line): return NetMessage(UIText(key: "%@", args: [line]))
            default: return NetMessage(UIText("paddock is not reachable"))
            }
        }
    }

    /// Removes rows. Rules of the app profile stay: paddock cannot remove
    /// them, a Deny rule overrides them.
    public func remove(_ rows: [RuleRow]) async {
        let ids = rows.filter { !$0.isProfile }.flatMap(\.ids)
        guard !ids.isEmpty else { return }
        guard let service else {
            problem = NetMessage(UIText("paddock is not reachable"))
            return
        }
        do {
            let result = try await service.decoded(NetworkArgs.ruleRemove(name, ids: ids), as: NetResultJSON.self)
            guard result.ok else {
                problem = NetMessage(UIText(key: "%@", args: [result.line ?? ""]))
                await loadRules(force: true)
                return
            }
            let gone = Set(result.removed ?? ids)
            rules.removeAll { gone.contains($0.id) }
            problem = nil
            for row in rows where !row.isProfile {
                store?.detail(for: name).recordMine("Removed rule " + NetRuleCopy.host(row).english)
            }
        } catch {
            problem = NetMessage(UIText(key: "%@", args: [LauncherStore.line(of: error)]))
        }
    }

    // MARK: Connections

    /// One click on a denied connection: an always-allow rule for the host
    /// and port, or for the whole site.
    public func allow(_ connection: ConnectionJSON, site: Bool) async {
        var draft = RuleDraft()
        draft.host = connection.host
        draft.site = site
        (draft.port, draft.otherPort) = PortChoice.of(site ? .any : .number(connection.port))
        if let message = await save(draft) {
            problem = message
            return
        }
        problem = nil
        let target = site ? (NetSite.registrable(connection.host) ?? connection.host) : connection.host
        for other in connections where other.isDenied && (site ? NetSite.registrable(other.host) == target
                                                              : other.host == connection.host) {
            allowed[other.id] = UIText("Allowed · rule added. The app connects on its next try.")
        }
    }

    // MARK: Clipboard

    /// The directions saved in the profile. While the app runs they apply
    /// to the session too.
    public func setClipboard(profile value: ClipboardJSON) async {
        await setClipboard(value, session: false)
    }

    /// The directions of the running session only. They end when it stops.
    public func setClipboard(session value: ClipboardJSON) async {
        await setClipboard(value, session: true)
    }

    func setClipboard(_ value: ClipboardJSON, session: Bool) async {
        guard let service, var state = clipboard else { return }
        let previous = state
        if session {
            state.session = value
        } else {
            state.profile = value
            if state.running { state.session = value }
        }
        clipboard = state
        do {
            let result = try await service.decoded(NetworkArgs.clipboard(name, value, session: session), as: NetResultJSON.self)
            guard result.ok else {
                clipboard = previous
                problem = NetMessage(UIText(key: "%@", args: [result.line ?? ""]))
                return
            }
            problem = nil
            let words = "to app \(value.toApp ? "on" : "off"), from app \(value.fromApp ? "on" : "off")"
            store?.detail(for: name).recordMine((session ? "Clipboard this session " : "Clipboard ") + words)
        } catch {
            clipboard = previous
            if NetLoad.of(error) == .needsNewerPaddock {
                problem = NetMessage(UIText("This paddock cannot change the clipboard of one session yet. Update paddock:"),
                                     command: ShellCommands.brewUpgrade)
            } else {
                problem = NetMessage(UIText(key: "%@", args: [LauncherStore.line(of: error)]))
            }
        }
    }

    // MARK: Folders

    /// `Keep for later starts` on a folder that only this session shares.
    public func keepFolder(_ path: String) async {
        guard let service else { return }
        do {
            let result = try await service.decoded(NetworkArgs.shareKeep(name, path: path), as: NetResultJSON.self)
            guard result.ok else {
                problem = NetMessage(UIText(key: "%@", args: [result.line ?? ""]))
                return
            }
            problem = nil
            let detail = store?.detail(for: name)
            if let index = detail?.shares.firstIndex(where: { $0.path == path }) { detail?.shares[index].from = "grant" }
            detail?.recordMine("Folder \(path) kept")
        } catch {
            problem = NetLoad.of(error) == .needsNewerPaddock
                ? NetMessage(UIText("This paddock cannot keep a session folder yet. Update paddock:"), command: ShellCommands.brewUpgrade)
                : NetMessage(UIText(key: "%@", args: [LauncherStore.line(of: error)]))
        }
    }

    /// The text next to a folder: its open files, before Remove.
    public static func openFiles(_ share: ShareJSON) -> UIText? {
        switch share.openHandles ?? 0 {
        case 0: return nil
        case 1: return UIText("1 file open")
        case let n: return UIText("%@ files open", "\(n)")
        }
    }
}
