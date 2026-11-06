import PaddockKit
import SwiftUI

/// The Grants tab: live permissions of one app, in the shape of System
/// Settings > Privacy & Security. Every row says whether a change is `Live`,
/// `On next start`, or needs the app to `Reopen files`.
@MainActor
struct GrantsView: View {
    let model: AppDetailModel
    @State private var folderSheet: FolderSheet?
    @State private var addingRule = false
    @State private var ruleSelection = Set<RuleJSON.ID>()
    @State private var confirmRuleDelete = false

    var body: some View {
        Form {
            FoldersSection(model: model, sheet: $folderSheet)
            networkSection
            clipboardSection
            DevicesSection(model: model)
            changesSection
        }
        .formStyle(.grouped)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("grants-form")
        .safeAreaInset(edge: .bottom, spacing: 0) { ToastBar(toast: model.toast) }
        .sheet(item: $folderSheet) { sheet in
            FolderSheetView(model: model, sheet: sheet) { folderSheet = $0 }
        }
        .sheet(isPresented: $addingRule) {
            AddRuleSheet(model: model)
        }
        .confirmationDialog(Text("Remove the selected rules?"), isPresented: $confirmRuleDelete) {
            Button("Remove", role: .destructive) {
                let hosts = model.rules.filter { ruleSelection.contains($0.id) }.map(\.host)
                Task { await model.removeRules(hosts) }
                ruleSelection = []
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The app asks again the next time it reaches these servers.")
        }
    }

    // MARK: Network

    private var networkSection: some View {
        Section("Network") {
            Picker("Mode", selection: Binding(get: { model.networkMode },
                                              set: { mode in Task { await model.setNetwork(mode) } })) {
                ForEach(NetworkMode.allCases) { mode in Text(mode.title).tag(mode) }
            }
            .accessibilityIdentifier("network-mode")
            if let text = model.status?.networkText {
                Text(verbatim: text)
            }
            if let note = model.networkNote {
                Text(note)
            }
            RulesTable(model: model, selection: $ruleSelection, confirmDelete: $confirmRuleDelete)
                .frame(minHeight: 160)
            HStack {
                Button("Add Rule…") { addingRule = true }
                    .accessibilityIdentifier("add-rule-button")
                Spacer()
                LiveLabel(label: .live)
            }
        }
    }

    // MARK: Clipboard

    private var clipboardSection: some View {
        Section("Clipboard") {
            Toggle(isOn: Binding(get: { model.clipboard.toApp },
                                 set: { on in Task { await model.setClipboard(toApp: on, fromApp: model.clipboard.fromApp) } })) {
                Text("Paste from this Mac into the app")
                Text("The app can read what you copy on this Mac when you paste in it.")
            }
            Toggle(isOn: Binding(get: { model.clipboard.fromApp },
                                 set: { on in Task { await model.setClipboard(toApp: model.clipboard.toApp, fromApp: on) } })) {
                Text("Copy from the app to this Mac")
                Text("What you copy in the app reaches the clipboard of this Mac.")
            }
            HStack {
                Spacer()
                LiveLabel(label: .live)
            }
        }
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
                    Text(verbatim: change.text)
                    Spacer()
                    Text(source)
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel(Text(verbatim: [Format.clock(change.time), change.text, source.localized]
                    .joined(separator: ", ")))
                .accessibilityIdentifier("change-row")
            }
        }
    }
}

/// The trailing status label of a grant row.
@MainActor
struct LiveLabel: View {
    let label: GrantLabel

    var body: some View {
        Text(label.text)
            .font(.caption)
            .accessibilityIdentifier("grant-label")
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
    @Binding var sheet: FolderSheet?

    var body: some View {
        Section {
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
            TimelineView(.periodic(from: .now, by: 1)) { context in
                ForEach(model.shares) { share in
                    FolderRow(model: model, share: share, now: context.date, sheet: $sheet)
                }
            }
            CommandEcho(args: PaddockArgs.shareAdd(model.name, path: "PATH", rw: false, unprotected: false))
            Button("Add Folder…") { addFolder() }
                .accessibilityIdentifier("add-folder-button")
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
    let share: ShareJSON
    let now: Date
    @Binding var sheet: FolderSheet?

    var body: some View {
        HStack {
            Image(systemName: "folder").accessibilityHidden(true)
            VStack(alignment: .leading) {
                Text(verbatim: Format.tildePath(share.path, home: NSHomeDirectory()))
                Text(OverviewCopy.used(share.lastUsed, now: now))
                    .font(.caption)
            }
            Spacer()
            Picker("Access", selection: Binding(
                get: { ShareMode(rawValue: share.mode) ?? .ro },
                set: { mode in Task { await model.setMode(share, mode) } })) {
                ForEach(ShareMode.allCases) { mode in Text(mode.title).tag(mode) }
            }
            .labelsHidden()
            .fixedSize()
            LiveLabel(label: model.label(for: share, now: now))
            Menu {
                Button("Reveal in Finder") { SystemLinks.reveal(share.path) }
                Button("Remove", role: .destructive) { remove() }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .accessibilityLabel(Text("More"))
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("folder-row")
    }

    private func remove() {
        Task {
            if case .confirm(let open) = await model.removeFolder(share) {
                sheet = .removeOpen(share, open)
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
