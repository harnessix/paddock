import Foundation

/// Helpers of the library actions.
public enum LibraryActions {
    /// A child of `app run` that ends later than this ran a session: its
    /// exit is the end of the session, which the event stream reports.
    public static let startWindow: TimeInterval = 60

    /// A non-zero exit of `app run` is a refused start only within the
    /// start window; later it is the end of a session.
    public static func isStartRefusal(status: Int32, elapsed: TimeInterval) -> Bool {
        status != 0 && elapsed < startWindow
    }

    /// paddock's own words for a failed call: the JSON `error`, else the last
    /// stderr line, else the last stdout line, else the exit status.
    public static func line(_ out: CommandOutput) -> String {
        struct ErrorOnly: Decodable { var error: String?; var text: String? }
        if let e = try? PaddockJSON.decoder.decode(ErrorOnly.self, from: out.stdout), let line = e.error ?? e.text,
           !line.isEmpty {
            return line
        }
        let err = CLIPaddockService.lastLine(out.stderr)
        if !err.isEmpty { return err }
        let std = CLIPaddockService.lastLine(out.stdoutText)
        if !std.isEmpty, !std.hasPrefix("{") { return std }
        return UIText("paddock ended with status %@.", "\(out.status)").english
    }

    /// Asks `paddock app --help` which optional commands exist.
    public static func probe(_ service: PaddockService) async -> Capabilities {
        guard let out = try? await service.raw(LibraryArgs.appHelp), out.status == 0 else { return Capabilities() }
        return Capabilities.parse(appHelp: out.stdoutText)
    }
}

extension LauncherStore {
    /// The instance of a sidebar key. A listed row knows its instance; an
    /// unknown key is read as `NAME--N`.
    public func instance(of key: String) -> InstanceRef {
        app(named: key)?.instance ?? InstanceRef(key: key)
    }

    /// The name on screen for a key.
    public func title(of key: String) -> String {
        app(named: key)?.title ?? InstanceRef(key: key).title(displayName: nil)
    }

    /// The instances of the app of `key`, in number order.
    public func instances(ofApp app: String) -> [AppItem] {
        apps.filter { $0.instance.app == app }.sorted { $0.instance.number < $1.instance.number }
    }

    // MARK: Sessions

    /// `Start` and `Resume`: `app run NAME [--instance N]`. paddock's `app
    /// run` has no `--json` and may stay until the session ends, so the
    /// launcher does not wait for it in the window. The state comes back on
    /// the event stream. A refusal is the child's exit within the first
    /// minute; its last line shows in place.
    public func launch(_ key: String) async {
        let ref = instance(of: key)
        await runSession(LibraryArgs.run(ref), errorKey: key, title: title(of: key))
    }

    /// `Open New Instance`: `app run NAME --new`.
    public func launchNewInstance(_ key: String) async {
        let ref = instance(of: key)
        await runSession(LibraryArgs.runNew(ref.app), errorKey: key, title: title(of: key))
        await reloadListNow()
    }

    func runSession(_ args: [String], errorKey: String, title: String) async {
        guard let service else { return }
        appErrors[errorKey] = nil
        let started = clock.now()
        do {
            let out = try await service.raw(args)
            let elapsed = clock.now().timeIntervalSince(started)
            if let result = try? PaddockJSON.decoder.decode(ResultJSON.self, from: out.stdout) {
                if !result.ok { appErrors[errorKey] = LauncherStore.startError(app: title, result: result) }
            } else if LibraryActions.isStartRefusal(status: out.status, elapsed: elapsed) {
                appErrors[errorKey] = .command(title: UIText("%@ did not start", title), line: LibraryActions.line(out))
            }
        } catch {
            appErrors[errorKey] = .command(title: UIText("%@ did not start", title), line: LauncherStore.line(of: error))
        }
    }

    /// `Stop`: `app stop NAME [--instance N]`. A suspended session stops at once.
    public func halt(_ key: String) async {
        guard let service else { return }
        let ref = instance(of: key)
        do {
            let out = try await service.raw(LibraryArgs.stop(ref))
            if out.status != 0 {
                appErrors[key] = .command(title: UIText("%@ did not stop", title(of: key)), line: LibraryActions.line(out))
            }
        } catch {
            appErrors[key] = .command(title: UIText("%@ did not stop", title(of: key)), line: LauncherStore.line(of: error))
        }
    }

    /// `Suspend`, only when this paddock has `app suspend`.
    public func suspend(_ key: String) async {
        guard let service, capabilities.suspend else { return }
        do {
            let out = try await service.raw(LibraryArgs.suspend(instance(of: key)))
            let result = try? PaddockJSON.decoder.decode(ResultJSON.self, from: out.stdout)
            if out.status != 0 || result?.ok == false {
                appErrors[key] = .command(title: UIText("%@ was not suspended", title(of: key)), line: LibraryActions.line(out))
            }
        } catch {
            appErrors[key] = .command(title: UIText("%@ was not suspended", title(of: key)), line: LauncherStore.line(of: error))
        }
    }

    /// Reloads the app list now, past the 5 s gate: after an add, a remove
    /// or a new instance the list must show it.
    public func reloadListNow() async {
        guard let service else { return }
        if let list = try? await service.listApps() { applyList(list) }
    }

    // MARK: Add

    /// The review of the Add App sheet: `app add PATH --dry-run --json`.
    public func review(path: String) async -> AddReview {
        guard binary.isUsable, let service else {
            return .failed(path: path, line: LibraryCopy.offline.english)
        }
        do {
            let out = try await service.raw(LibraryArgs.review(path))
            if let report = LibraryDecode.addReport(out.stdout) { return .of(report: report, path: path) }
            if let precheck = try? PaddockJSON.decoder.decode(PrecheckJSON.self, from: out.stdout) {
                return .of(precheck: precheck, path: path)
            }
            return .failed(path: path, line: LibraryActions.line(out))
        } catch {
            return .failed(path: path, line: LauncherStore.line(of: error))
        }
    }

    /// `Add`: `app add`, then `app install` for the Dock and Launchpad
    /// launcher, then the new row is selected.
    public func add(path: String, name: String, memoryGB: Int) async -> AddOutcome {
        guard let service else { return .refused(LibraryCopy.offline.english) }
        do {
            let out = try await service.raw(LibraryArgs.add(path, name: name, memoryGB: memoryGB))
            var profile = name
            var launchpad: String?
            if let report = LibraryDecode.addReport(out.stdout) {
                guard report.ok == true, report.error == nil else {
                    return .refused(report.refusals?.first?.text ?? report.error ?? LibraryActions.line(out))
                }
                profile = report.existing ?? report.name ?? name
                launchpad = (report.displayName ?? name) + " (paddock)"
            } else if let result = try? PaddockJSON.decoder.decode(ResultJSON.self, from: out.stdout) {
                guard result.ok else { return .refused(result.error ?? result.message ?? LibraryActions.line(out)) }
                launchpad = result.launchpadName
            } else if out.status != 0 {
                return .refused(LibraryActions.line(out))
            }
            if let install = try? await service.raw(PaddockArgs.install(profile)), install.status != 0 {
                show(UIText("%@ added. The Launchpad launcher was not made: %@", name, LibraryActions.line(install)))
            } else {
                show(UIText("%@ added. It also appears in Launchpad as \"%@\".", name, launchpad ?? "\(name) (paddock)"))
            }
            await reloadListNow()
            selection = .app(profile)
            detailTab = .overview
            return .added(key: profile)
        } catch {
            return .refused(LauncherStore.line(of: error))
        }
    }

    // MARK: Remove

    /// `Remove` in the remove sheet. The data goes to paddock's trash for
    /// 7 days unless `deleteData`.
    @discardableResult
    public func remove(_ key: String, deleteData: Bool) async -> UIText? {
        guard let service else { return nil }
        let ref = instance(of: key)
        let title = self.title(of: key)
        do {
            let out = try await service.raw(LibraryArgs.remove(ref.app, deleteData: deleteData))
            let report = try? PaddockJSON.decoder.decode(RemoveReportJSON.self, from: out.stdout)
            let result = try? PaddockJSON.decoder.decode(ResultJSON.self, from: out.stdout)
            if out.status != 0 || report?.error != nil || result?.ok == false {
                lastError = .command(title: UIText("%@ was not removed", title),
                                     line: report?.error ?? result?.error ?? LibraryActions.line(out))
                return nil
            }
            let freed = report?.freed ?? result?.freedBytes ?? 0
            let text = LibraryCopy.removed(title, freed: freed, trashBytes: report?.trashBytes ?? 0)
            apps.removeAll { $0.instance.app == ref.app }
            details = details.filter { InstanceRef(key: $0.key).app != ref.app }
            if case .app(let selected)? = selection, InstanceRef(key: selected).app == ref.app || app(named: selected) == nil {
                selection = apps.first.map { .app($0.key) }
            }
            show(text)
            return text
        } catch {
            lastError = .command(title: UIText("%@ was not removed", title), line: LauncherStore.line(of: error))
            return nil
        }
    }
}
