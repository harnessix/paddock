import PaddockKit
import SwiftUI

/// The parts of the Grants tab.
enum GrantsPart: String, CaseIterable, Identifiable {
    case network, connections, files

    var id: String { rawValue }

    var title: UIText {
        switch self {
        case .network: return UIText("Network")
        case .connections: return UIText("Connections")
        case .files: return UIText("Files & Clipboard")
        }
    }
}

/// The Grants tab: the live permissions of one app, in three parts.
/// Network has the mode and the rules, Connections the recent connections
/// of the session, Files & Clipboard the folders, the clipboard, the
/// devices and the changes. Every row says whether a change is `Live`,
/// `On next start`, or needs the app to `Reopen files`.
@MainActor
struct GrantsView: View {
    let model: AppDetailModel
    @Environment(LauncherStore.self) private var store
    @State private var part: GrantsPart = .network
    @State private var folderSheet: FolderSheet?

    var body: some View {
        let net = NetworkModels.model(for: model.name, store: store)
        VStack(spacing: 0) {
            Picker("Show", selection: $part) {
                ForEach(GrantsPart.allCases) { item in Text(item.title).tag(item) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(maxWidth: .infinity)
            .padding(.top, 10)
            .accessibilityIdentifier("grants-part")
            switch part {
            case .network:
                NetworkRulesPane(model: model, net: net)
            case .connections:
                ConnectionsPane(net: net)
            case .files:
                filesForm(net)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) { ToastBar(toast: model.toast) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Grants"))
        // A grant change from elsewhere (a panel answer, the CLI) reaches
        // the Changes list first: reload the rules then, through the 5 s
        // gate. A change of this window already updated the list.
        .onChange(of: model.changes.first?.id) { _, _ in
            if model.changes.first?.source != "you" {
                Task { await net.loadRules() }
            }
        }
        .sheet(item: $folderSheet) { sheet in
            FolderSheetView(model: model, sheet: sheet) { folderSheet = $0 }
        }
    }

    private func filesForm(_ net: NetworkModel) -> some View {
        Form {
            if let problem = net.problem {
                NetProblemBanner(message: problem) { net.problem = nil }
            }
            FoldersSection(model: model, net: net, sheet: $folderSheet)
            ClipboardSections(net: net)
            DevicesSection(model: model)
            changesSection
        }
        .formStyle(.grouped)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("grants-form")
    }

    // MARK: Changes

    private var changesSection: some View {
        Section("Changes") {
            if model.changes.isEmpty {
                Text("No changes yet")
            }
            ForEach(model.changes) { change in
                let source = AppDetailModel.sourceText(change.source)
                HStack {
                    Text(verbatim: Format.clock(change.time))
                        .monospacedDigit()
                        .labelInk()
                    Text(verbatim: change.text).labelInk()
                    Spacer()
                    Text(source).labelInk()
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel(Text(verbatim: [Format.clock(change.time), change.text, source.localized]
                    .joined(separator: ", ")))
                .accessibilityIdentifier("change-row")
            }
        }
    }
}

/// The trailing status of a grant row, on a gray capsule: `Live`, `On next
/// start`, or `Reopen files`. `Reopen files` is the one that asks the user
/// to act: an orange capsule with a symbol, and it fades back to `Live`
/// after 30 s. The text stays in the label color for contrast.
@MainActor
struct LiveLabel: View {
    let label: GrantLabel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 4) {
            if label == .reopenFiles {
                Image(systemName: "arrow.clockwise")
                    .foregroundStyle(.orange)
                    .accessibilityHidden(true)
                    .transition(.opacity)
            }
            Text(label.text)
                .labelInk()
                .accessibilityIdentifier("grant-label")
        }
        .font(.caption)
        .lineLimit(1)
        .padding(.horizontal, 7)
        .padding(.vertical, 2)
        .background(capsule, in: Capsule())
        .fixedSize()
        .animation(reduceMotion ? nil : .easeOut(duration: 0.6), value: label)
    }

    private var capsule: AnyShapeStyle {
        label == .reopenFiles ? AnyShapeStyle(Color.orange.opacity(0.22)) : AnyShapeStyle(.fill.quaternary)
    }
}

// MARK: - Folders

enum FolderSheet: Identifiable, Equatable {
    case refused(String)
    case unprotected(path: String, line: String)
    case removeOpen(ShareJSON, Int)

    var id: String {
        switch self {
        case .refused(let line): return "refused:" + line
        case .unprotected(let path, _): return "unprotected:" + path
        case .removeOpen(let share, _): return "remove:" + share.path
        }
    }
}

@MainActor
struct FoldersSection: View {
    let model: AppDetailModel
    let net: NetworkModel
    @Binding var sheet: FolderSheet?

    var body: some View {
        Section {
            Button("Add Folder…") { addFolder() }
                .accessibilityIdentifier("add-folder-button")
            if let downloads = model.downloads {
                HStack {
                    Image(systemName: "arrow.down.circle").accessibilityHidden(true)
                    Text(OverviewCopy.downloads(downloads, home: NSHomeDirectory()))
                    Spacer()
                    Button("Change…") {
                        if let path = Pickers.folder(start: downloads) { Task { await model.setDownloads(path) } }
                    }
                    Button("Reveal") { SystemLinks.reveal(downloads) }
                    LiveLabel(label: .live)
                }
            }
            if let error = model.loadError {
                HStack(alignment: .firstTextBaseline) {
                    ProblemLine { Text(verbatim: error).fixedSize(horizontal: false, vertical: true) }
                    Spacer()
                    Button("Try Again") { Task { await model.load(force: true) } }
                }
                .accessibilityIdentifier("folders-error")
            } else if model.status == nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading folders…")
                }
            } else if model.shares.isEmpty {
                Text("No folders yet. The app sees none of your files.")
                    .accessibilityIdentifier("folders-empty")
            }
            TimelineView(.periodic(from: .now, by: 1)) { context in
                ForEach(model.shares) { share in
                    FolderRow(model: model, net: net, share: share, now: context.date, sheet: $sheet)
                }
            }
            CommandEcho(args: PaddockArgs.shareAdd(model.name, path: "PATH", rw: false, unprotected: false))
        } header: {
            Text("Folders")
        } footer: {
            Text("The app sees only these folders. They appear in the app under \"My Shared Files\".")
        }
    }

    private func addFolder() {
        guard let path = Pickers.folder() else { return }
        Task {
            switch await model.addFolder(path) {
            case .refused(let line), .failed(let line): sheet = .refused(line)
            case .needsUnprotected(let line): sheet = .unprotected(path: path, line: line)
            case .added: break
            }
        }
    }
}

@MainActor
struct FolderRow: View {
    let model: AppDetailModel
    let net: NetworkModel
    let share: ShareJSON
    let now: Date
    @Binding var sheet: FolderSheet?

    var body: some View {
        HStack {
            Image(systemName: "folder").accessibilityHidden(true)
            VStack(alignment: .leading) {
                Text(verbatim: Format.tildePath(share.path, home: NSHomeDirectory())).labelInk()
                // The open files show before Remove, not only in its sheet.
                Text(verbatim: caption)
                    .font(.caption)
                    .labelInk()
                    .accessibilityIdentifier("folder-caption")
            }
            Spacer()
            Picker("Access", selection: Binding(
                get: { ShareMode(rawValue: share.mode) ?? .ro },
                set: { mode in Task { await model.setMode(share, mode) } })) {
                ForEach(ShareMode.allCases) { mode in Text(mode.title).tag(mode) }
            }
            .labelsHidden()
            .menuAuditAction()
            .accessibilityIdentifier("folder-access")
            LiveLabel(label: model.label(for: share, now: now))
            Menu {
                Button("Reveal in Finder") { SystemLinks.reveal(share.path) }
                if share.from == "run" {
                    Button("Keep for Later Starts") { Task { await net.keepFolder(share.path) } }
                }
                Divider()
                Button("Remove", role: .destructive) { remove() }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .menuAuditAction()
            .accessibilityLabel(Text("More"))
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text(verbatim: Format.tildePath(share.path, home: NSHomeDirectory())))
        .accessibilityIdentifier("folder-row")
    }

    /// `Used 3 min ago · 2 files open · This session only`.
    private var caption: String {
        var parts = [OverviewCopy.used(share.lastUsed, now: now).localized]
        if let open = NetworkModel.openFiles(share) { parts.append(open.localized) }
        if share.from == "run" { parts.append(UIText("This session only").localized) }
        return parts.joined(separator: " · ")
    }

    private func remove() {
        Task {
            switch await model.removeFolder(share) {
            case .confirm(let open): sheet = .removeOpen(share, open)
            case .failed(let line) where !line.isEmpty: net.problem = NetMessage(UIText(key: "%@", args: [line]))
            default: break
            }
        }
    }
}

/// The sheets of the Folders section: paddock's refusal line, the `.env`
/// question, and removal with open files.
@MainActor
struct FolderSheetView: View {
    let model: AppDetailModel
    let sheet: FolderSheet
    let next: (FolderSheet?) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            switch sheet {
            case .refused(let line):
                Label { Text("This folder cannot be shared").font(.headline) } icon: {
                    Image(systemName: "nosign").foregroundStyle(.red).accessibilityHidden(true)
                }
                Text(verbatim: line).accessibilityIdentifier("refusal-line")
                HStack {
                    Spacer()
                    Button("OK") { dismiss() }.keyboardShortcut(.defaultAction)
                }
            case .unprotected(let path, let line):
                Label { Text("This folder holds keys").font(.headline) } icon: {
                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange).accessibilityHidden(true)
                }
                Text(verbatim: line).accessibilityIdentifier("refusal-line")
                CommandEcho(args: PaddockArgs.shareAdd(model.name, path: path, rw: false, unprotected: true))
                HStack {
                    Spacer()
                    Button("Cancel", role: .cancel) { dismiss() }.keyboardShortcut(.cancelAction)
                    Button("Share anyway") {
                        Task {
                            let outcome = await model.addFolder(path, unprotected: true)
                            dismiss()
                            if case .refused(let line) = outcome { next(.refused(line)) }
                        }
                    }
                    .accessibilityIdentifier("share-anyway")
                }
            case .removeOpen(let share, let open):
                Text(AppDetailModel.removalText(app: model.name, openFiles: open,
                                                path: Format.tildePath(share.path, home: NSHomeDirectory())))
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("remove-folder-text")
                HStack {
                    Spacer()
                    Button("Cancel", role: .cancel) { dismiss() }.keyboardShortcut(.cancelAction)
                    Button("Remove", role: .destructive) {
                        Task {
                            _ = await model.removeFolder(share, confirmed: true)
                            dismiss()
                        }
                    }
                }
            }
        }
        .padding(20)
        .frame(width: 440)
    }
}

// MARK: - Devices

@MainActor
struct DevicesSection: View {
    let model: AppDetailModel

    var body: some View {
        Section {
            ForEach(model.devices) { device in
                HStack {
                    Toggle(isOn: Binding(get: { device.enabled },
                                         set: { on in Task { await model.setDevice(device, on: on) } })) {
                        Text(verbatim: device.name)
                    }
                    .accessibilityIdentifier("device-" + device.name)
                    LiveLabel(label: .nextStart)
                }
            }
            if model.deviceChangedWhileRunning {
                Button("Restart App") { Task { await model.restart() } }
                    .accessibilityIdentifier("restart-app")
            }
        } header: {
            Text("Devices")
        } footer: {
            Text("Devices attach when the app starts.")
        }
    }
}
