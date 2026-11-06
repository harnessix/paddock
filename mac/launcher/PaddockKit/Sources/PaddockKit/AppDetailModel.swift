import Foundation
import Observation

/// The trailing status label of a grant row.
public enum GrantLabel: Equatable, Sendable {
    case live, nextStart, reopenFiles

    public var text: UIText {
        switch self {
        case .live: return UIText("Live")
        case .nextStart: return UIText("On next start")
        case .reopenFiles: return UIText("Reopen files")
        }
    }
}

/// What happened to `Add Folder…`.
public enum FolderOutcome: Equatable, Sendable {
    case added(ShareJSON)
    /// paddock refused the folder. The sheet shows the line verbatim.
    case refused(line: String)
    /// The folder holds a `.env` or key file. The sheet shows the line
    /// with `Cancel` / `Share anyway`.
    case needsUnprotected(line: String)
    case failed(line: String)
}

/// What happened to `Remove` on a folder row.
public enum FolderRemoval: Equatable, Sendable {
    case removed
    /// The app has files open there: ask first.
    case confirm(openFiles: Int)
    case failed(line: String)
}

public enum HostCheck: Equatable, Sendable {
    case ok(String)
    case invalid
    case everyHost
}

/// The model of an app's detail tabs: Overview, Grants and Log.
@MainActor
@Observable
public final class AppDetailModel {
    public let name: String
    public var status: AppStatusJSON?
    public var shares: [ShareJSON] = []
    public var rules: [RuleJSON] = []
    public var clipboard = ClipboardJSON(toApp: true, fromApp: true)
    public var devices: [DeviceJSON] = []
    public var changes: [ChangeJSON] = []
    public var downloads: String?
    /// Folders that show `Reopen files` until the given time.
    public var reopenUntil: [String: Date] = [:]
    /// True after a device change while the app runs: shows `Restart App`.
    public var deviceChangedWhileRunning = false
    /// `Open connections close now.` after a switch to No network.
    public var networkNote: UIText?
    public var toast: Toast?
    public var loadError: String?
    public var logLines: [String] = []
    public var following = false

    @ObservationIgnored weak var store: LauncherStore?
    @ObservationIgnored var followTask: Task<Void, Never>?
    @ObservationIgnored var toastTask: Task<Void, Never>?
    public static let reopenSeconds: Double = 30
    public static let logLimit = 500
    public static let changesLimit = 20

    init(name: String, store: LauncherStore) {
        self.name = name
        self.store = store
    }

    var service: PaddockService? { store?.service }
    var clock: LauncherClock? { store?.clock }

    public var app: AppItem? { store?.app(named: name) }
    public var isRunning: Bool { app?.state.isActive ?? false }

    // MARK: Load

    /// Runs `app status --json`, at most once per 5 s.
    public func load(force: Bool = false) async {
        guard let store, let service else { return }
        guard store.gate.allow("app status " + name, now: store.clock.now()) || (force && status == nil) else { return }
        do {
            let status = try await service.appStatus(name)
            applyStatus(status)
            loadError = nil
        } catch {
            loadError = LauncherStore.line(of: error)
        }
    }

    public func applyStatus(_ status: AppStatusJSON) {
        self.status = status
        shares = status.shares ?? []
        rules = status.rules ?? []
        clipboard = status.clipboard ?? clipboard
        devices = status.devices ?? []
        changes = Array((status.changes ?? []).sorted { $0.time > $1.time }.prefix(AppDetailModel.changesLimit))
        downloads = status.downloads
        if let index = store?.apps.firstIndex(where: { $0.name == name }) {
            store?.apps[index] = AppItem(json: status.app)
        }
    }

    func sessionChanged(_ app: AppItem) {
        if !app.state.isActive { deviceChangedWhileRunning = false }
    }

    /// A grant event from the stream: add it to Changes, then reload the
    /// grants (the gate keeps that to one call per 5 s).
    func apply(_ event: GrantEvent) {
        let time = event.time ?? clock?.now() ?? Date()
        let change = ChangeJSON(time: time, text: event.detail, source: event.source)
        if !isDuplicate(change) { record(change) }
        Task { await self.load() }
    }

    func isDuplicate(_ change: ChangeJSON) -> Bool {
        changes.contains { $0.text == change.text && $0.source == change.source
            && abs($0.time.timeIntervalSince(change.time)) < 10 }
    }

    func record(_ change: ChangeJSON) {
        changes.insert(change, at: 0)
        if changes.count > AppDetailModel.changesLimit { changes.removeLast(changes.count - AppDetailModel.changesLimit) }
    }

    func recordMine(_ text: String) {
        record(ChangeJSON(time: clock?.now() ?? Date(), text: text, source: "you"))
    }

    func showToast(_ text: UIText) {
        let toast = Toast(text)
        self.toast = toast
        toastTask?.cancel()
        toastTask = Task { [weak self] in
            try? await self?.clock?.sleep(seconds: 6)
            if !Task.isCancelled, self?.toast?.id == toast.id { self?.toast = nil }
        }
    }

    public static func sourceText(_ source: String) -> UIText {
        switch source {
        case "you": return UIText("You")
        case "question_panel": return UIText("Question panel")
        case "profile": return UIText("Profile")
        default: return UIText(key: "%@", args: [source])
        }
    }

    // MARK: Folders

    public func label(for share: ShareJSON, now: Date) -> GrantLabel {
        if let until = reopenUntil[share.path], now < until { return .reopenFiles }
        switch share.applies {
        case "next_start": return .nextStart
        case "reopen": return .reopenFiles
        case "live": return .live
        default: return isRunning ? .live : .nextStart
        }
    }

    public func addFolder(_ path: String, readWrite: Bool = false, unprotected: Bool = false) async -> FolderOutcome {
        guard let service else { return .failed(line: "") }
        do {
            let result = try await service.perform(PaddockArgs.shareAdd(name, path: path, rw: readWrite, unprotected: unprotected))
            if let line = result.refused { return .refused(line: line) }
            if result.needsUnprotected == true { return .needsUnprotected(line: result.message ?? "") }
            guard result.ok else { return .failed(line: result.error ?? result.message ?? "") }
            var share = result.share ?? ShareJSON(path: path, mode: readWrite ? "rw" : "ro")
            share.applies = result.applies ?? share.applies
            shares.removeAll { $0.path == share.path }
            shares.append(share)
            let home = NSHomeDirectory()
            let shown = Format.tildePath(share.path, home: home)
            let mode = share.mode == "rw" ? UIText("read and write") : UIText("read only")
            let guest = share.guestPath ?? "/Volumes/My Shared Files/" + URL(fileURLWithPath: path).lastPathComponent
            showToast(UIText("Added %@, %@. Visible in the app under %@.", shown, mode.english, guest))
            recordMine("Folder \(shown) \(share.mode)")
            return .added(share)
        } catch {
            return .failed(line: LauncherStore.line(of: error))
        }
    }

    public func setMode(_ share: ShareJSON, _ mode: ShareMode) async {
        guard let service, share.mode != mode.rawValue else { return }
        do {
            let result = try await service.perform(PaddockArgs.shareMode(name, path: share.path, mode: mode))
            guard result.ok else {
                showToast(UIText(key: "%@", args: [result.error ?? result.refused ?? ""]))
                return
            }
            let handles = result.openHandles ?? share.openHandles ?? 0
            if let index = shares.firstIndex(where: { $0.path == share.path }) {
                shares[index].mode = mode.rawValue
                shares[index].openHandles = handles
                if let applies = result.applies { shares[index].applies = applies }
            }
            if share.mode == "rw", mode == .ro, handles > 0, let clock {
                let until = clock.now().addingTimeInterval(AppDetailModel.reopenSeconds)
                reopenUntil[share.path] = until
                // The deadline is fixed now. The task can start later, after
                // the clock moved, so it waits only for the time that is left.
                Task { [weak self] in
                    try? await clock.sleep(seconds: until.timeIntervalSince(clock.now()))
                    self?.expireReopen()
                }
            }
            recordMine("Folder \(share.path) \(mode.rawValue)")
        } catch {
            showToast(UIText(key: "%@", args: [LauncherStore.line(of: error)]))
        }
    }

    /// Drops the `Reopen files` labels whose 30 s are over.
    public func expireReopen() {
        guard let now = clock?.now() else { return }
        reopenUntil = reopenUntil.filter { $0.value > now }
    }

    /// `Remove` on a folder row. With open files the first call only asks;
    /// `confirmed` removes.
    public func removeFolder(_ share: ShareJSON, confirmed: Bool = false) async -> FolderRemoval {
        let open = share.openHandles ?? 0
        if open > 0, isRunning, !confirmed { return .confirm(openFiles: open) }
        guard let service else { return .failed(line: "") }
        do {
            let result = try await service.perform(PaddockArgs.shareRemove(name, path: share.path))
            guard result.ok else { return .failed(line: result.error ?? "") }
            shares.removeAll { $0.path == share.path }
            reopenUntil[share.path] = nil
            recordMine("Folder \(share.path) removed")
            return .removed
        } catch {
            return .failed(line: LauncherStore.line(of: error))
        }
    }

    public static func removalText(app: String, openFiles: Int, path: String) -> UIText {
        if openFiles == 1 {
            return UIText("%@ has 1 file open in %@. Removing the folder closes it for the app. Unsaved changes in that file can be lost.", app, path)
        }
        return UIText("%@ has %@ files open in %@. Removing the folder closes them for the app. Unsaved changes in those files can be lost.",
                      app, "\(openFiles)", path)
    }

    public func setDownloads(_ path: String) async {
        guard let service else { return }
        if let result = try? await service.perform(PaddockArgs.downloads(name, path: path)), result.ok {
            downloads = path
            recordMine("Downloads \(path)")
        }
    }

    // MARK: Network

    public var networkMode: NetworkMode {
        NetworkMode(rawValue: app?.json.network ?? status?.app.network ?? "ask") ?? .ask
    }

    public func setNetwork(_ mode: NetworkMode) async {
        guard let service, mode != networkMode else { return }
        do {
            try await service.execute(PaddockArgs.network(name, mode))
            if let index = store?.apps.firstIndex(where: { $0.name == name }) {
                var json = store!.apps[index].json
                json.network = mode.rawValue
                store?.apps[index] = AppItem(json: json)
            }
            networkNote = (mode == .none && isRunning && (status?.openConnections ?? 0) > 0)
                ? UIText("Open connections close now.") : nil
            recordMine("Network \(mode.rawValue)")
        } catch {
            showToast(UIText(key: "%@", args: [LauncherStore.line(of: error)]))
        }
    }

    public static func checkHost(_ text: String) -> HostCheck {
        let host = text.trimmingCharacters(in: .whitespaces).lowercased()
        if host == "*" || host == "*.*" { return .everyHost }
        var labels = host.split(separator: ".", omittingEmptySubsequences: false)
        if labels.first == "*" { labels.removeFirst() }
        guard labels.count >= 2 else { return .invalid }
        let allowed = Set("abcdefghijklmnopqrstuvwxyz0123456789-")
        for label in labels {
            guard !label.isEmpty, label.count <= 63, label.allSatisfy({ allowed.contains($0) }),
                  !label.hasPrefix("-"), !label.hasSuffix("-") else { return .invalid }
        }
        return .ok(host)
    }

    /// `Add Rule…`. Returns the inline message of the sheet, or nil when the
    /// rule was added.
    public func addRule(host text: String, answer: RuleAnswer, scope: RuleScope) async -> UIText? {
        let check = AppDetailModel.checkHost(text)
        if check == .invalid { return UIText("Enter a host name, for example api.example.com.") }
        guard let service else { return nil }
        let host = text.trimmingCharacters(in: .whitespaces).lowercased()
        do {
            let result = try await service.perform(PaddockArgs.ruleAdd(name, host: host, answer: answer, scope: scope))
            if let line = result.refused { return UIText(key: "%@", args: [line]) }
            if check == .everyHost, !result.ok {
                return UIText(key: "%@", args: [ShellCommands.allHostsRule(app: name)])
            }
            guard result.ok else { return UIText(key: "%@", args: [result.error ?? ""]) }
            let rule = result.rule ?? RuleJSON(host: host, answer: answer.rawValue, scope: scope.rawValue,
                                               added: clock?.now(), count: 0)
            rules.removeAll { $0.host == rule.host && $0.seeded != true }
            rules.append(rule)
            recordMine(RuleText.describe(answer: answer, scope: scope, host: host))
            return nil
        } catch {
            return UIText(key: "%@", args: [LauncherStore.line(of: error)])
        }
    }

    public func removeRules(_ hosts: [String]) async {
        guard let service else { return }
        for host in hosts {
            guard let rule = rules.first(where: { $0.host == host }), rule.seeded != true else { continue }
            if let result = try? await service.perform(PaddockArgs.ruleRemove(name, host: host)), result.ok {
                rules.removeAll { $0.host == host && $0.seeded != true }
                recordMine("Removed rule \(host)")
            }
        }
    }

    /// `Change to Deny` / `Change to Allow`: a new rule replaces the old.
    public func flipRule(_ rule: RuleJSON) async {
        let answer: RuleAnswer = rule.answer == "allow" ? .deny : .allow
        _ = await addRule(host: rule.host, answer: answer, scope: RuleScope(rawValue: rule.scope) ?? .always)
    }

    // MARK: Clipboard and devices

    public func setClipboard(toApp: Bool, fromApp: Bool) async {
        guard let service else { return }
        let previous = clipboard
        clipboard = ClipboardJSON(toApp: toApp, fromApp: fromApp)
        do {
            try await service.execute(PaddockArgs.clipboard(name, toApp: toApp, fromApp: fromApp))
            recordMine("Clipboard to app \(toApp ? "on" : "off"), from app \(fromApp ? "on" : "off")")
        } catch {
            clipboard = previous
            showToast(UIText(key: "%@", args: [LauncherStore.line(of: error)]))
        }
    }

    public func setDevice(_ device: DeviceJSON, on: Bool) async {
        guard let service else { return }
        do {
            try await service.execute(PaddockArgs.device(name, name: device.name, on: on))
            if let index = devices.firstIndex(where: { $0.name == device.name }) {
                devices[index].enabled = on
                devices[index].applies = "next_start"
            }
            if isRunning { deviceChangedWhileRunning = true }
            recordMine("Device \(device.name) \(on ? "on" : "off")")
        } catch {
            showToast(UIText(key: "%@", args: [LauncherStore.line(of: error)]))
        }
    }

    /// `Restart App`: stop, wait for the saved state, start again.
    public func restart() async {
        guard let store, let clock else { return }
        await store.stopApp(name)
        let deadline = clock.now().addingTimeInterval(120)
        while clock.now() < deadline, let app = store.app(named: name), app.state.isActive {
            try? await clock.sleep(seconds: 0.5)
        }
        deviceChangedWhileRunning = false
        await store.startApp(name)
    }

    /// `Clear` in the clear-saved-state sheet.
    public func clearSavedState() async {
        guard let service else { return }
        if let result = try? await service.perform(PaddockArgs.clearSavedState(name)), result.ok {
            store?.show(UIText("Saved state of %@ cleared.", name))
            await load(force: true)
        }
    }

    public static func clearText(app: String) -> UIText {
        UIText("Clear the saved state of %@? The next start is cold, about 30 s. Your data in the app stays.", app)
    }

    // MARK: Log

    public func loadLog() async {
        guard let service else { return }
        if let text = try? await service.text(PaddockArgs.log(name)) {
            logLines = Array(text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
                .suffix(AppDetailModel.logLimit))
        }
    }

    public func setFollow(_ on: Bool) {
        following = on
        followTask?.cancel()
        followTask = nil
        guard on, let service else { return }
        let stream = service.lines(PaddockArgs.logFollow(name))
        followTask = Task { [weak self] in
            do {
                for try await line in stream {
                    guard let self else { return }
                    self.logLines.append(line)
                    if self.logLines.count > AppDetailModel.logLimit {
                        self.logLines.removeFirst(self.logLines.count - AppDetailModel.logLimit)
                    }
                }
            } catch {}
        }
    }
}

/// Texts of the rules list.
public enum RuleText {
    public static func describe(answer: RuleAnswer, scope: RuleScope, host: String) -> String {
        switch (answer, scope) {
        case (.allow, .always): return "Allow always \(host)"
        case (.allow, .session): return "Allow this session \(host)"
        case (.deny, .always): return "Deny always \(host)"
        case (.deny, .session): return "Deny this session \(host)"
        }
    }

    public static func answer(_ rule: RuleJSON) -> UIText {
        rule.answer == "allow" ? UIText("Allow") : UIText("Deny")
    }

    public static func scope(_ rule: RuleJSON) -> UIText {
        rule.scope == "session" ? UIText("This session") : UIText("Always")
    }

    public static func flipTitle(_ rule: RuleJSON) -> UIText {
        rule.answer == "allow" ? UIText("Change to Deny") : UIText("Change to Allow")
    }
}
