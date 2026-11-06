import AppKit
import PaddockKit
import SwiftUI

/// Add App: pick an `.app`, show paddock's precheck, then add it.
@MainActor
struct AddAppSheet: View {
    let initialPath: String?
    @Environment(LauncherStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var path: String?
    @State private var precheck: PrecheckJSON?
    @State private var verdict: AddVerdict?
    @State private var name = ""
    @State private var memory = 6
    @State private var network: NetworkMode = LauncherDefaults.defaultNetwork
    @State private var adding = false

    var body: some View {
        Form {
            if let path {
                Section {
                    HStack(spacing: 12) {
                        AppIcon(path: path, size: 48)
                        VStack(alignment: .leading) {
                            Text(verbatim: precheck?.name ?? (path as NSString).lastPathComponent).font(.headline)
                            if let version = precheck?.version { Text(verbatim: version) }
                            if let size = precheck?.sizeBytes { Text(verbatim: Format.gb(size)) }
                        }
                    }
                }
                verdictSection
            } else {
                Section { ProgressView().accessibilityLabel(Text("Checking…")) }
            }
        }
        .formStyle(.grouped)
        .frame(width: 480)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Add") { add() }
                    .disabled(verdict?.canAdd != true || name.trimmingCharacters(in: .whitespaces).isEmpty || adding)
                    .accessibilityIdentifier("add-confirm")
            }
        }
        .task {
            guard path == nil else { return }
            guard let picked = initialPath ?? Pickers.app() else { return dismiss() }
            path = picked
            let (json, result) = await store.precheck(path: picked)
            precheck = json
            verdict = result
            name = json?.name ?? ((picked as NSString).lastPathComponent as NSString).deletingPathExtension
            memory = memoryOptions.contains(6) ? 6 : (memoryOptions.last ?? 4)
        }
    }

    /// 4, 6 or 8 GB, at most half of this Mac's memory.
    private var memoryOptions: [Int] {
        let half = Int(ProcessInfo.processInfo.physicalMemory / 2 / 1_073_741_824)
        let options = (precheck?.memoryOptionsGb ?? [4, 6, 8]).filter { $0 <= max(4, half) }
        return options.isEmpty ? [4] : options
    }

    @ViewBuilder private var verdictSection: some View {
        switch verdict {
        case .ok?:
            Section {
                TextField("Name", text: $name).accessibilityIdentifier("add-name")
                Picker("Memory", selection: $memory) {
                    ForEach(memoryOptions, id: \.self) { gb in Text(verbatim: Format.memoryGB(Int64(gb) * 1_073_741_824)).tag(gb) }
                }
                Picker("Network", selection: $network) {
                    ForEach(NetworkMode.allCases) { Text($0.title).tag($0) }
                }
                CommandEcho(args: PaddockArgs.add(path ?? "", name: name, memoryGB: memory, network: network))
            }
        case .appStore(let website)?:
            Section {
                ProblemLine {
                    Text(AddVerdict.appStore(website: website).message!)
                        .accessibilityIdentifier("add-message")
                }
                if let website, let url = URL(string: website) {
                    Button("Open website") { SystemLinks.open(url) }
                }
            }
        case .noGolden?:
            Section {
                Text(AddVerdict.noGolden.message!).accessibilityIdentifier("add-message")
                Button("Build…") {
                    store.selection = .baseImage
                    dismiss()
                }
                .accessibilityIdentifier("add-build")
            }
        case .lowSpace(let need, let free)?:
            Section {
                Text(AddVerdict.lowSpace(needBytes: need, freeBytes: free).message!).accessibilityIdentifier("add-message")
                Button("Manage Storage…") { SystemLinks.manageStorage() }
            }
        case .failed(let line)?:
            Section { Text(verbatim: line).accessibilityIdentifier("add-message") }
        case nil:
            Section { ProgressView().accessibilityLabel(Text("Checking…")) }
        }
    }

    private func add() {
        guard let path else { return }
        adding = true
        Task {
            let ok = await store.addApp(path: path, name: name.trimmingCharacters(in: .whitespaces),
                                        memoryGB: memory, network: network)
            adding = false
            if ok { dismiss() }
        }
    }
}

/// Remove App: names the sizes, data goes to the Trash by default.
@MainActor
struct RemoveAppSheet: View {
    let name: String
    @Environment(LauncherStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var deleteData = false
    @State private var working = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let app = store.app(named: name) {
                let sheet = RemovePresentation.of(app)
                Text(sheet.title).font(.headline)
                Text(sheet.body)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("remove-body")
                Toggle("Also delete the data now instead of moving it to the Trash", isOn: $deleteData)
                    .accessibilityIdentifier("remove-delete-data")
                CommandEcho(args: PaddockArgs.remove(name, deleteData: deleteData))
                if sheet.blockedByRunning {
                    HStack {
                        Text(RemoveCopy.stopFirst)
                        Button("Stop") { Task { await store.stopApp(name) } }
                    }
                }
                HStack {
                    Spacer()
                    Button("Cancel", role: .cancel) { dismiss() }
                        .keyboardShortcut(.cancelAction)
                    Button("Remove", role: .destructive) {
                        working = true
                        Task {
                            await store.removeApp(name, deleteData: deleteData)
                            dismiss()
                        }
                    }
                    .disabled(sheet.blockedByRunning || working)
                    .accessibilityIdentifier("remove-confirm")
                }
            }
        }
        .padding(20)
        .frame(width: 480)
    }
}

/// Stop with open questions: the one case that asks first.
@MainActor
struct ConfirmStopSheet: View {
    let name: String
    @Environment(LauncherStore.self) private var store
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ConfirmLayout(title: Text(UIText("Stop %@?", name)),
                      message: Text("Stopping saves the app's state so it reopens in about 10 s. Its open questions are denied."),
                      action: Text("Stop"), destructive: false) {
            await store.stopApp(name)
        }
    }
}

@MainActor
struct ClearStateSheet: View {
    let name: String
    @Environment(LauncherStore.self) private var store

    var body: some View {
        ConfirmLayout(title: Text(AppDetailModel.clearText(app: name)), message: nil, action: Text("Clear"), destructive: true) {
            await store.detail(for: name).clearSavedState()
        }
    }
}

@MainActor
struct RemoveGoldenSheet: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        if let info = store.golden.info {
            ConfirmLayout(title: Text(BuildCopy.removeQuestion(info)), message: nil, action: Text("Remove"), destructive: true) {
                _ = await store.removeGolden()
            }
        }
    }
}

@MainActor
struct RebuildSheet: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        ConfirmLayout(title: Text(BuildCopy.rebuildQuestion), message: nil, action: Text("Rebuild"), destructive: false) {
            store.build(rebuild: true)
            store.selection = .baseImage
        }
    }
}

@MainActor
struct CancelBuildSheet: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        ConfirmLayout(title: Text(BuildCopy.cancelQuestion), message: nil, action: Text("Stop"), destructive: true,
                      cancelTitle: Text("Keep Building")) {
            store.cancelBuild()
        }
    }
}

/// A sheet with a question, Cancel and one action. The action is never the
/// default button when it is destructive; Escape cancels.
@MainActor
struct ConfirmLayout: View {
    let title: Text
    let message: Text?
    let action: Text
    let destructive: Bool
    var cancelTitle = Text("Cancel")
    let perform: () async -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var working = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            title.font(.headline).fixedSize(horizontal: false, vertical: true)
            if let message { message.fixedSize(horizontal: false, vertical: true) }
            HStack {
                Spacer()
                Button(role: .cancel) { dismiss() } label: { cancelTitle }
                    .keyboardShortcut(.cancelAction)
                if destructive {
                    Button(role: .destructive) { run() } label: { action }
                        .disabled(working)
                        .accessibilityIdentifier("confirm-action")
                } else {
                    Button { run() } label: { action }
                        .keyboardShortcut(.defaultAction)
                        .disabled(working)
                        .accessibilityIdentifier("confirm-action")
                }
            }
        }
        .padding(20)
        .frame(width: 440)
    }

    private func run() {
        working = true
        Task {
            await perform()
            dismiss()
        }
    }
}
