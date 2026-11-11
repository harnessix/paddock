import Foundation
import Observation

/// One row of the storage table: an item of the macvm folder and its size.
public struct StorageRow: Equatable, Sendable, Identifiable {
    public enum Kind: String, Equatable, Sendable {
        case golden, goldenPrev, goldenNew, restoreImages, trash, agents, shims, instance
    }

    public var kind: Kind
    public var title: UIText
    /// A second line: the parts of an instance's size.
    public var detail: UIText?
    /// `Running`, `Suspended`, `Stopped`, or nil for files.
    public var state: UIText?
    public var bytes: Int64
    public var instance: InstanceRef?
    /// An item paddock must not remove now.
    public var inUse: Bool

    public var id: String { kind == .instance ? "instance:" + (instance?.key ?? "") : kind.rawValue }
}

/// The Storage pane: the total, the free space and the rows, from
/// `paddock du --json`.
public struct StorageReport: Equatable, Sendable {
    public var root: String
    public var totalBytes: Int64
    public var freeBytes: Int64?
    public var rows: [StorageRow]

    public var isEmpty: Bool { rows.allSatisfy { $0.bytes == 0 } }

    public var usageLine: UIText {
        if let free = freeBytes {
            return UIText("paddock uses %@. %@ free on this disk.", Format.gb(totalBytes), Format.gb(free))
        }
        return UIText("paddock uses %@.", Format.gb(totalBytes))
    }

    /// nil when paddock has no macvm folder yet.
    public static func of(_ du: DuReportJSON, apps: [AppItem], freeBytes: Int64?) -> StorageReport? {
        guard let m = du.macvm else { return nil }
        var rows: [StorageRow] = []
        func file(_ kind: StorageRow.Kind, _ title: UIText, _ bytes: Int64?, detail: UIText? = nil) {
            guard let bytes, bytes > 0 else { return }
            rows.append(StorageRow(kind: kind, title: title, detail: detail, state: nil, bytes: bytes, instance: nil, inUse: false))
        }
        file(.golden, UIText("Base image"), m.golden)
        file(.goldenPrev, UIText("Previous base image"), m.goldenPrev,
             detail: UIText("Kept until an app ran on the new base image. Clean Up removes it then."))
        file(.goldenNew, UIText("New base image"), m.goldenNew, detail: UIText("The update that builds now."))
        file(.restoreImages, UIText("Restore images"), m.ipsw, detail: UIText("The macOS download. Needed again only for a rebuild."))
        file(.trash, UIText("Trash"), m.trash, detail: UIText("Data of removed apps, kept for 7 days."))
        file(.agents, UIText("Guest helpers"), m.agent)
        file(.shims, UIText("Dock apps"), m.shims)
        for p in m.profiles ?? [] {
            let ref = InstanceRef(app: p.app, number: p.instance)
            let display = apps.first { $0.instance.app == p.app }?.json.displayName
            let parts = UIText("Saved state %@ · Backups %@", Format.gb(p.savedState ?? 0), Format.gb(p.backups ?? 0))
            rows.append(StorageRow(kind: .instance, title: UIText(key: "%@", args: [ref.title(displayName: display)]),
                                   detail: parts, state: StorageReport.stateText(p.state), bytes: p.bytes,
                                   instance: ref, inUse: p.state == "running" || p.state == "suspended"))
        }
        rows.sort { $0.bytes == $1.bytes ? $0.id < $1.id : $0.bytes > $1.bytes }
        return StorageReport(root: m.root, totalBytes: m.bytes, freeBytes: freeBytes, rows: rows)
    }

    public static func stateText(_ state: String) -> UIText {
        switch state {
        case "running": return UIText("Running")
        case "suspended": return UIText("Suspended")
        default: return UIText("Stopped")
        }
    }
}

/// The preview of `paddock gc --json` in two groups: what `Remove` takes
/// now and what stays until the option is on.
public struct CleanupPreview: Equatable, Sendable {
    public struct Item: Equatable, Sendable, Identifiable {
        public var item: GcItemJSON
        /// True for an item of the macvm folder, false for other paddock data.
        public var isMac: Bool
        public var removed: Bool
        public var id: String { item.what + "|" + (item.action?.dir ?? "") }
    }

    public var includeLogins: Bool
    public var items: [Item]
    public var notes: [String]

    /// The flags of `gc` that this preview ran with.
    public var flags: [String] { includeLogins ? ["--homes", "--include-app-logins"] : [] }

    public var removedNow: [Item] { items.filter(\.removed) }
    public var kept: [Item] { items.filter { !$0.removed } }
    public var bytesNow: Int64 { removedNow.reduce(0) { $0 + $1.item.bytes } }
    public var isEmpty: Bool { removedNow.isEmpty }

    /// True when the list has items outside the macvm folder: the sheet
    /// names them, because `gc --yes` takes them too.
    public var hasOtherData: Bool { removedNow.contains { !$0.isMac } }

    public var removeTitle: UIText { UIText("Remove %@", Format.gb(bytesNow)) }

    public static func of(_ gc: GcReportJSON, includeLogins: Bool) -> CleanupPreview {
        let given = Set(includeLogins ? ["--homes", "--include-app-logins"] : [])
        let items = gc.items.map { item -> Item in
            let needs = Set(item.needs ?? [])
            let removed = item.tier == "auto" ? needs.isSubset(of: given) : (!needs.isEmpty && needs.isSubset(of: given))
            return Item(item: item, isMac: item.action?.kind == "macvm", removed: removed)
        }
        return CleanupPreview(includeLogins: includeLogins, items: items, notes: gc.notes ?? [])
    }
}

/// What `gc --yes` did.
public struct CleanupResult: Equatable, Sendable {
    public struct Failure: Equatable, Sendable, Identifiable {
        public var what: String
        public var line: String
        public var id: String { what }
    }

    public var freedBytes: Int64
    public var failures: [Failure]

    public var line: UIText {
        failures.isEmpty ? UIText("%@ freed.", Format.gb(freedBytes))
            : UIText("%@ freed. %@ items were not removed.", Format.gb(freedBytes), "\(failures.count)")
    }

    public static func of(_ gc: GcReportJSON) -> CleanupResult {
        var freed: Int64 = 0
        var failures: [Failure] = []
        for row in gc.removed ?? [] {
            if let error = row.error, !error.isEmpty {
                failures.append(Failure(what: row.what, line: error))
            } else {
                freed += row.bytes ?? 0
            }
        }
        return CleanupResult(freedBytes: freed, failures: failures)
    }
}

/// The state of the Storage pane and its Clean Up sheet.
@MainActor
@Observable
public final class StorageModel {
    public enum Phase: Equatable, Sendable {
        case idle
        case loading
        case loaded(StorageReport)
        /// paddock has no macvm folder yet.
        case empty
        case failed(String)
        /// No usable paddock.
        case offline
    }

    public enum CleanupPhase: Equatable, Sendable {
        case closed
        case previewing
        case preview(CleanupPreview)
        case removing(CleanupPreview)
        case done(CleanupResult)
        case failed(String)
    }

    public var phase: Phase = .idle
    public var cleanup: CleanupPhase = .closed
    public var includeLogins = false

    public init() {}

    public func load(_ store: LauncherStore) async {
        guard store.binary.isUsable, let service = store.service else {
            phase = .offline
            return
        }
        if case .loaded = phase {} else { phase = .loading }
        do {
            let out = try await service.raw(LibraryArgs.du)
            guard out.status == 0 else {
                phase = .failed(LibraryActions.line(out))
                return
            }
            let du = try PaddockJSON.decoder.decode(DuReportJSON.self, from: out.stdout)
            if let report = StorageReport.of(du, apps: store.apps, freeBytes: store.disk?.freeBytes) {
                phase = report.isEmpty ? .empty : .loaded(report)
            } else {
                phase = .empty
            }
        } catch {
            phase = .failed(LauncherStore.line(of: error))
        }
    }

    /// `Clean Up…` and the logins option: a dry run of `gc`.
    public func preview(_ store: LauncherStore) async {
        guard let service = store.service else { return }
        cleanup = .previewing
        do {
            let out = try await service.raw(LibraryArgs.gc(apply: false, includeLogins: includeLogins))
            guard let gc = try? PaddockJSON.decoder.decode(GcReportJSON.self, from: out.stdout) else {
                cleanup = .failed(LibraryActions.line(out))
                return
            }
            cleanup = .preview(CleanupPreview.of(gc, includeLogins: includeLogins))
        } catch {
            cleanup = .failed(LauncherStore.line(of: error))
        }
    }

    /// `Remove` in the sheet: `gc --yes` with the flags of the preview.
    public func apply(_ store: LauncherStore) async {
        guard let service = store.service, case .preview(let preview) = cleanup else { return }
        cleanup = .removing(preview)
        do {
            let out = try await service.raw(LibraryArgs.gc(apply: true, includeLogins: preview.includeLogins))
            guard let gc = try? PaddockJSON.decoder.decode(GcReportJSON.self, from: out.stdout) else {
                cleanup = .failed(LibraryActions.line(out))
                return
            }
            let result = CleanupResult.of(gc)
            cleanup = .done(result)
            store.show(result.line)
        } catch {
            cleanup = .failed(LauncherStore.line(of: error))
        }
        await load(store)
    }

    public func closeCleanup() {
        cleanup = .closed
        includeLogins = false
    }
}
