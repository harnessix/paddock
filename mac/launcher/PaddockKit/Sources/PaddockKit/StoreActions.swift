import Foundation

/// The actions of the launcher. Each one is one paddock call (or a short
/// fixed sequence). The state change itself comes back on the event stream.
extension LauncherStore {
    // MARK: Apps

    /// `Start`. paddock refuses a start that cannot work; the refusal shows
    /// in place before anything starts.
    public func startApp(_ name: String) async {
        guard let service else { return }
        appErrors[name] = nil
        do {
            let result = try await service.perform(PaddockArgs.run(name))
            if !result.ok { appErrors[name] = LauncherStore.startError(app: name, result: result) }
        } catch {
            appErrors[name] = .command(title: UIText("%@ did not start", name), line: LauncherStore.line(of: error))
        }
    }

    public static func startError(app: String, result: ResultJSON) -> LauncherError {
        switch result.code {
        case "two_vms":
            return .twoRunning(app: app, running: result.running ?? [])
        case "low_disk":
            return .lowDiskAtStart(app: app, needBytes: result.needBytes ?? 0, freeBytes: result.freeBytes ?? 0)
        case "no_golden":
            return .goldenRemoved
        default:
            return .command(title: UIText("%@ did not start", app), line: result.error ?? result.message ?? "")
        }
    }

    public func stopApp(_ name: String) async {
        guard let service else { return }
        do {
            try await service.execute(PaddockArgs.stop(name))
        } catch {
            appErrors[name] = .command(title: UIText("%@ did not stop", name), line: LauncherStore.line(of: error))
        }
    }

    /// `Open`: brings the running app's windows forward.
    public func openApp(_ name: String) async {
        try? await service?.execute(PaddockArgs.open(name))
    }

    /// The row click of the menu bar extra: the first enabled of Open and Start.
    public func primaryAction(_ name: String) async {
        guard let app = app(named: name) else { return }
        if app.state.isRunning {
            await openApp(name)
        } else if app.state.canStart {
            await startApp(name)
        }
    }

    /// ⌘⇧S: Start or Stop the selected app.
    public func toggleSelected() async {
        guard let app = selectedApp else { return }
        if app.state.canStop {
            await stopApp(app.name)
        } else if app.state.canStart {
            await startApp(app.name)
        }
    }

    // MARK: Questions

    /// Answers a question. The panel next to the app's window closes too:
    /// paddock owns it, the launcher only sends the answer.
    public func answer(_ question: QuestionJSON, _ answer: Answer) async {
        guard let service else { return }
        do {
            try await service.execute(PaddockArgs.answer(question.id, answer))
            questions.removeAll { $0.id == question.id }
        } catch {
            lastError = .command(title: UIText("The answer did not reach paddock"), line: LauncherStore.line(of: error))
        }
    }

    public func pausedDenyAll() async {
        guard let service, let paused else { return }
        try? await service.execute(PaddockArgs.pausedDenyAll(paused.app))
        questions.removeAll { $0.app == paused.app }
        self.paused = nil
    }

    public func pausedOneByOne() async {
        guard let service, let paused else { return }
        try? await service.execute(PaddockArgs.pausedOneByOne(paused.app))
        self.paused = nil
    }

    // MARK: Base image

    /// Runs `macvm setup --json` as a child. Quitting the launcher pauses
    /// it, closing the window does not.
    public func build(dir: String? = nil, ipsw: String? = nil, resume: Bool = false, rebuild: Bool = false) {
        guard let service, !buildRunning else { return }
        buildRunning = true
        if case .building = golden {} else { golden = .building(BuildProgress()) }
        let args = PaddockArgs.setup(dir: dir, ipsw: ipsw, resume: resume, rebuild: rebuild)
        buildTask = Task { [weak self] in
            do {
                for try await line in service.setup(args) {
                    self?.applySetup(line)
                }
            } catch {
                if let self, case .building = self.golden {
                    self.golden = .failed(BuildFailure(step: nil, code: "exited", reason: LauncherStore.line(of: error)))
                }
            }
            self?.buildRunning = false
            await self?.reloadStatus()
        }
    }

    /// `Pause`: the download step only.
    public func pauseBuild() async {
        try? await service?.execute(PaddockArgs.setupPause)
        if case .building(var progress) = golden {
            progress.paused = true
            progress.rateSince = nil
            golden = .building(progress)
        }
    }

    /// `Resume` or `Try Again`: continues from paddock's manifest.
    public func resumeBuild() {
        if case .building(var progress) = golden {
            progress.paused = false
            golden = .building(progress)
        }
        if buildRunning {
            Task { try? await self.service?.execute(PaddockArgs.setup(dir: nil, ipsw: nil, resume: true)) }
        } else {
            build(resume: true)
        }
    }

    /// `Stop` in the cancel sheet. The downloaded part stays.
    public func cancelBuild() {
        buildTask?.cancel()
        buildTask = nil
        buildRunning = false
        Task { await self.reloadStatus() }
    }

    public func showSetupWindow() async {
        try? await service?.execute(PaddockArgs.setupShowWindow)
    }

    /// `Check for newer macOS`.
    public func checkLatest() async {
        guard let service else { return }
        do {
            let result = try await service.perform(PaddockArgs.macvmLatest)
            if let latest = result.latest, let current = result.current ?? golden.info?.version, latest != current {
                latestAvailable = latest
                latestCheck = UIText("macOS %@ is available. Apps keep running on %@ until you rebuild.", latest, current)
            } else {
                latestAvailable = nil
                latestCheck = UIText("Up to date")
            }
        } catch {
            latestCheck = UIText(key: "%@", args: [LauncherStore.line(of: error)])
        }
    }

    /// `Remove Base Image…` then `Remove`.
    public func removeGolden() async -> Int64? {
        guard let service else { return nil }
        do {
            let result = try await service.perform(PaddockArgs.macvmRemove)
            if result.ok {
                golden = .none
                show(UIText("Base image removed. %@ freed.", Format.gb(result.freedBytes ?? 0)))
                return result.freedBytes ?? 0
            }
            lastError = .command(title: UIText("The base image was not removed"), line: result.error ?? "")
        } catch {
            lastError = .command(title: UIText("The base image was not removed"), line: LauncherStore.line(of: error))
        }
        return nil
    }

    // MARK: Remove app

    /// `Remove` in the remove sheet. Data goes to the Trash unless
    /// `deleteData` is on.
    @discardableResult
    public func removeApp(_ name: String, deleteData: Bool) async -> Int64? {
        guard let service else { return nil }
        do {
            let result = try await service.perform(PaddockArgs.remove(name, deleteData: deleteData))
            guard result.ok else {
                lastError = .command(title: UIText("%@ was not removed", name), line: result.error ?? "")
                return nil
            }
            apps.removeAll { $0.name == name }
            details[name] = nil
            if selection == .app(name) { selection = apps.first.map { .app($0.name) } }
            let freed = result.freedBytes ?? 0
            show(UIText("%@ removed. %@ freed.", name, Format.gb(freed)))
            return freed
        } catch {
            lastError = .command(title: UIText("%@ was not removed", name), line: LauncherStore.line(of: error))
            return nil
        }
    }

    // MARK: Helpers

    /// paddock's own words for a failed call.
    public static func line(of error: Error) -> String {
        switch error {
        case CommandError.failed(_, let line): return line
        case CommandError.badJSON(let output): return output
        case CommandError.notFound: return UIText("paddock is not installed").english
        default: return String(describing: error)
        }
    }
}
