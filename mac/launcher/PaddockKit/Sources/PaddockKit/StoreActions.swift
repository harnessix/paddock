import Foundation

/// The actions of the launcher. Each one is one paddock call (or a short
/// fixed sequence). The state change itself comes back on the event stream.
extension LauncherStore {
    // MARK: Apps

    /// `Start`. paddock refuses a start that cannot work; the refusal shows
    /// in place before anything starts.
    /// `Start` and `Resume`. `name` is a sidebar key: the profile name,
    /// or `NAME--N` for instance N. A refusal shows in place.
    public func startApp(_ name: String) async {
        await launch(name)
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
        await halt(name)
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
        } else if app.state.canStart || app.state.canResume {
            await startApp(name)
        }
    }

    /// ⌘⇧S: Start or Stop the selected app.
    public func toggleSelected() async {
        guard let app = selectedApp else { return }
        if app.state.canStop {
            await stopApp(app.key)
        } else if app.state.canStart {
            await startApp(app.key)
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
    /// paddock's setup has no `--dir`: the base image stays in the folder of
    /// `macvm status`. A rebuild is `macvm update` (`updateGolden`).
    public func build(dir: String? = nil, ipsw: String? = nil, resume: Bool = false, rebuild: Bool = false) {
        if rebuild {
            updateGolden()
            return
        }
        guard let service, !buildRunning else { return }
        buildRunning = true
        buildNote = nil
        if case .building = golden {} else { golden = .building(BuildProgress()) }
        let args = LibraryArgs.setup(ipsw: ipsw, resume: resume)
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
        await requestPause()
    }

    /// `Resume` or `Try Again`: continues from paddock's manifest.
    public func resumeBuild() {
        Task { await self.requestResume() }
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
    public func removeGolden() async -> Bool {
        await removeBaseImage()
    }

    // MARK: Remove app

    /// `Remove` in the remove sheet. Data goes to paddock's trash unless
    /// `deleteData` is on.
    @discardableResult
    public func removeApp(_ name: String, deleteData: Bool) async -> UIText? {
        await remove(name, deleteData: deleteData)
    }

    // MARK: Helpers

    /// paddock's own words for a failed call.
    public nonisolated static func line(of error: Error) -> String {
        switch error {
        case CommandError.failed(_, let line): return line
        case CommandError.badJSON(let output): return output
        case CommandError.notFound: return UIText("paddock is not installed").english
        default: return String(describing: error)
        }
    }
}
