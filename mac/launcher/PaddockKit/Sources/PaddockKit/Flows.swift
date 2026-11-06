import Foundation

// MARK: - Add app

/// What the Add App sheet shows after paddock's precheck.
public enum AddVerdict: Equatable, Sendable {
    case ok
    case appStore(website: String?)
    case noGolden
    case lowSpace(needBytes: Int64, freeBytes: Int64)
    case failed(line: String)

    public static func of(_ precheck: PrecheckJSON) -> AddVerdict {
        if precheck.appStore == true { return .appStore(website: precheck.website) }
        if let golden = precheck.golden, golden != "ready", golden != "needs_update" { return .noGolden }
        if let need = precheck.needBytes, let free = precheck.freeBytes, free < need {
            return .lowSpace(needBytes: need, freeBytes: free)
        }
        if !precheck.ok { return .failed(line: precheck.message ?? "") }
        return .ok
    }

    public var canAdd: Bool { self == .ok }

    public var message: UIText? {
        switch self {
        case .ok: return nil
        case .appStore:
            return UIText("This app came from the App Store and needs an Apple ID, which the sandbox does not have. Install the version from the maker's website instead.")
        case .noGolden: return UIText("Build the base image first.")
        case .lowSpace(let need, let free): return UIText("Needs %@, %@ free.", Format.gb(need), Format.gb(free))
        case .failed(let line): return UIText(key: "%@", args: [line])
        }
    }
}

extension LauncherStore {
    public func precheck(path: String) async -> (PrecheckJSON?, AddVerdict) {
        guard let service else { return (nil, .failed(line: UIText("paddock is not installed").english)) }
        do {
            let precheck = try await service.precheck(path)
            return (precheck, AddVerdict.of(precheck))
        } catch {
            return (nil, .failed(line: LauncherStore.line(of: error)))
        }
    }

    /// `Add`: adds the profile, installs the `(paddock).app` launcher,
    /// selects the new row and shows the toast.
    public func addApp(path: String, name: String, memoryGB: Int, network: NetworkMode) async -> Bool {
        guard let service else { return false }
        do {
            let result = try await service.perform(PaddockArgs.add(path, name: name, memoryGB: memoryGB, network: network))
            guard result.ok else {
                lastError = .command(title: UIText("%@ was not added", name), line: result.error ?? result.message ?? "")
                return false
            }
            try? await service.execute(PaddockArgs.install(name))
            if let list = try? await service.listApps() { applyList(list) }
            selection = .app(name)
            detailTab = .overview
            let launchpad = result.launchpadName ?? "\(name) (paddock)"
            show(UIText("%@ added. It also appears in Launchpad as \"%@\".", name, launchpad))
            return true
        } catch {
            lastError = .command(title: UIText("%@ was not added", name), line: LauncherStore.line(of: error))
            return false
        }
    }
}

// MARK: - Remove app

public struct RemovePresentation: Equatable, Sendable {
    public var title: UIText
    public var body: UIText
    public var blockedByRunning: Bool

    public static func of(_ app: AppItem) -> RemovePresentation {
        let sizes = app.json.sizes ?? AppSizesJSON(appCopy: 0, data: 0, savedState: 0)
        return RemovePresentation(
            title: UIText("Remove %@ from paddock?", app.name),
            body: UIText("This removes the sandbox copy of the app (%@), its saved state (%@) and the launcher in your Applications folder. Its data (%@) goes to the Trash, where you can get it back. The original app in /Applications stays.",
                         Format.gb(sizes.appCopy), Format.gb(sizes.savedState), Format.gb(sizes.data)),
            blockedByRunning: app.state.isActive)
    }
}

// MARK: - Uninstall

public struct UninstallPlan: Equatable, Sendable {
    public var removeApps = true
    public var removeGolden = true
    public var removeData = false

    public init(removeApps: Bool = true, removeGolden: Bool = true, removeData: Bool = false) {
        self.removeApps = removeApps
        self.removeGolden = removeGolden
        self.removeData = removeData
    }

    /// The sizes next to the three checkboxes.
    public static func sizes(apps: [AppItem], golden: GoldenState) -> (apps: Int64, golden: Int64, data: Int64) {
        let appBytes = apps.reduce(Int64(0)) { $0 + ($1.json.sizes.map { $0.appCopy + $0.savedState } ?? 0) }
        let dataBytes = apps.reduce(Int64(0)) { $0 + ($1.json.sizes?.data ?? 0) }
        return (appBytes, golden.info?.sizeBytes ?? 0, dataBytes)
    }

    /// The paddock calls in order: apps first (with their data when that
    /// box is on, else the data goes to the Trash), then the base image.
    /// `brew` is never called: removing the paddock command stays a
    /// command the user copies.
    public func commands(apps: [String]) -> [[String]] {
        var list: [[String]] = []
        if removeApps {
            for app in apps { list.append(PaddockArgs.remove(app, deleteData: removeData)) }
        }
        if removeGolden { list.append(PaddockArgs.macvmRemove) }
        return list
    }
}

extension LauncherStore {
    /// Runs the uninstall plan and returns the freed bytes.
    public func uninstall(_ plan: UninstallPlan) async -> Int64 {
        guard let service else { return 0 }
        var freed: Int64 = 0
        for app in activeApps { await stopApp(app.name) }
        for args in plan.commands(apps: apps.map(\.name)) {
            if let result = try? await service.perform(args), result.ok {
                freed += result.freedBytes ?? 0
            }
        }
        if plan.removeApps {
            apps = []
            details = [:]
            selection = nil
        }
        if plan.removeGolden { golden = .none }
        return freed
    }
}

// MARK: - Quit

public enum QuitAction: Equatable, Sendable {
    case pauseBuild
    case stop(String)
}

public enum QuitPlanner {
    /// What quitting the launcher does. With `keepRunning` on (the
    /// default) apps keep running and nothing is sent. A running build is
    /// paused either way: it is a child of the launcher.
    public static func plan(keepRunning: Bool, apps: [AppItem], building: Bool) -> [QuitAction] {
        var actions: [QuitAction] = []
        if building { actions.append(.pauseBuild) }
        if !keepRunning {
            for app in apps where app.state.isActive { actions.append(.stop(app.name)) }
        }
        return actions
    }
}

extension LauncherStore {
    public var isBuilding: Bool {
        if case .building = golden, buildRunning { return true }
        return false
    }

    /// Runs the quit plan. Waits for each stopped app to reach `saved`, up
    /// to `timeout` seconds in all.
    public func performQuit(keepRunning: Bool, timeout: Double = 60) async {
        let plan = QuitPlanner.plan(keepRunning: keepRunning, apps: apps, building: isBuilding)
        var stopping: [String] = []
        for action in plan {
            switch action {
            case .pauseBuild:
                await pauseBuild()
                buildTask?.cancel()
            case .stop(let name):
                await stopApp(name)
                stopping.append(name)
            }
        }
        guard !stopping.isEmpty else { return }
        let deadline = clock.now().addingTimeInterval(timeout)
        while clock.now() < deadline {
            let waiting = stopping.filter { name in
                guard let app = app(named: name) else { return false }
                if case .saved = app.state { return false }
                if case .stopped = app.state { return false }
                return true
            }
            if waiting.isEmpty { return }
            try? await clock.sleep(seconds: 0.25)
        }
    }

    /// The line under `Quit paddock for Mac`, or nil when no app runs.
    public func quitNote(keepRunning: Bool) -> UIText? {
        let count = activeApps.count
        guard count > 0 else { return nil }
        if keepRunning {
            return count == 1 ? UIText("1 app keeps running") : UIText("%@ apps keep running", "\(count)")
        }
        return count == 1 ? UIText("Quitting stops 1 app") : UIText("Quitting stops %@ apps", "\(count)")
    }
}

// MARK: - Menu bar extra

public struct MenuModel: Equatable, Sendable {
    public static let maxApps = 8

    public var header: UIText
    public var apps: [AppItem]
    public var showsMore: Bool
    public var questionsItem: UIText?
    public var quitNote: UIText?
    /// `running`, `question` or `idle`: picks the icon variant.
    public var iconState: IconState

    public enum IconState: Equatable, Sendable {
        case idle, running, question
    }

    @MainActor
    public static func of(_ store: LauncherStore, keepRunning: Bool) -> MenuModel {
        let count = store.questions.count
        let icon: IconState = count > 0 ? .question : (store.activeApps.isEmpty ? .idle : .running)
        return MenuModel(
            header: store.golden.menuHeader,
            apps: Array(store.apps.prefix(maxApps)),
            showsMore: store.apps.count > maxApps,
            questionsItem: count > 0 ? UIText("Open questions… (%@)", "\(count)") : nil,
            quitNote: store.quitNote(keepRunning: keepRunning),
            iconState: icon)
    }
}

// MARK: - Notifications

public enum QuestionNotifier {
    /// The questions that waited longer than `threshold` and got no
    /// notification yet. One notification per question.
    public static func due(_ questions: [QuestionJSON], now: Date, threshold: TimeInterval?,
                           notified: Set<String>) -> [QuestionJSON] {
        guard let threshold else { return [] }
        return questions.filter { q in
            guard !notified.contains(q.id), let opened = q.opened else { return false }
            return now.timeIntervalSince(opened) >= threshold
        }
    }

    public static func title(_ question: QuestionJSON) -> UIText {
        UIText("%@ is waiting for an answer", question.app)
    }
}
