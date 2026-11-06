import AppKit
import PaddockKit
import ServiceManagement
import SwiftUI

/// Settings (⌘,): General, Storage, Network, Advanced, in a sidebar.
@MainActor
struct SettingsView: View {
    enum Pane: String, CaseIterable, Identifiable {
        case general, storage, network, advanced
        var id: String { rawValue }
    }

    @State private var pane: Pane? = .general

    var body: some View {
        NavigationSplitView {
            List(selection: $pane) {
                Label { Text("General") } icon: { DecorativeSymbol(name: "gearshape") }.tag(Pane.general)
                Label { Text("Storage") } icon: { DecorativeSymbol(name: "internaldrive") }.tag(Pane.storage)
                Label { Text("Network") } icon: { DecorativeSymbol(name: "network") }.tag(Pane.network)
                Label { Text("Advanced") } icon: { DecorativeSymbol(name: "wrench.and.screwdriver") }.tag(Pane.advanced)
            }
            .listStyle(.sidebar)
            .navigationSplitViewColumnWidth(170)
        } detail: {
            switch pane ?? .general {
            case .general: GeneralPane()
            case .storage: StoragePane()
            case .network: NetworkPane()
            case .advanced: AdvancedPane()
            }
        }
    }
}

@MainActor
struct GeneralPane: View {
    @AppStorage(SettingsKey.showInMenuBar, store: LauncherDefaults.store) private var showInMenuBar = true
    @AppStorage(SettingsKey.keepRunning, store: LauncherDefaults.store) private var keepRunning = true
    @AppStorage(SettingsKey.notifyAfter, store: LauncherDefaults.store) private var notifyAfter = 30
    @State private var openAtLogin = SMAppService.mainApp.status == .enabled
    @State private var loginError: String?

    var body: some View {
        Form {
            Section {
                Toggle("Open paddock for Mac at login", isOn: $openAtLogin)
                    .onChange(of: openAtLogin) { _, on in setLogin(on) }
                if let loginError { ProblemLine { Text(verbatim: loginError) } }
                Toggle(isOn: $showInMenuBar) {
                    Text("Show in menu bar")
                    Text("When this is off, open paddock for Mac from the Applications folder to get the window and this setting back.")
                }
                Toggle(isOn: $keepRunning) {
                    Text("Keep running apps when paddock for Mac quits")
                    Text("When this is off, quitting stops each app and saves its state.")
                }
                Picker("Notify when a question waits longer than", selection: $notifyAfter) {
                    Text("30 s").tag(30)
                    Text("2 min").tag(120)
                    Text("Never").tag(0)
                }
            }
            Section {
                LabeledContent("Language") {
                    Text("paddock for Mac uses the language of macOS.")
                }
            }
        }
        .formStyle(.grouped)
    }

    private func setLogin(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
            loginError = nil
        } catch {
            loginError = error.localizedDescription
            openAtLogin = SMAppService.mainApp.status == .enabled
        }
    }
}

@MainActor
struct StoragePane: View {
    @Environment(LauncherStore.self) private var store
    @State private var confirmClear = false

    var body: some View {
        Form {
            if let disk = store.disk, let total = disk.totalBytes {
                let used = disk.usedByPaddockBytes ?? 0
                Section {
                    ProgressView(value: Double(used), total: Double(max(total, 1)))
                        .accessibilityLabel(Text(StorageCopy.usage(used: used, total: total)))
                    Text(StorageCopy.usage(used: used, total: total))
                }
            }
            Section {
                if let info = store.golden.info {
                    LabeledContent("Base image") { Text(verbatim: Format.gb(info.sizeBytes)) }
                }
                ForEach(store.apps) { app in
                    if let sizes = app.json.sizes {
                        LabeledContent {
                            Text(UIText("App copy %@ · Data %@ · Saved state %@", Format.gb(sizes.appCopy),
                                        Format.gb(sizes.data), Format.gb(sizes.savedState)))
                        } label: {
                            Text(verbatim: app.name)
                        }
                    }
                }
                if let ipsw = store.disk?.ipswBytes, ipsw > 0 {
                    LabeledContent("Restore image download") {
                        HStack {
                            Text(verbatim: Format.gb(ipsw))
                            Button("Delete") { Task { try? await store.service?.execute(PaddockArgs.deleteRestoreImage) } }
                                .disabled(!store.golden.isReady)
                        }
                    }
                }
                if let trash = store.disk?.trashBytes {
                    LabeledContent("Trash of removed apps") {
                        HStack {
                            Text(verbatim: Format.gb(trash))
                            Button("Reveal") { SystemLinks.reveal(store.disk?.trashDir ?? NSHomeDirectory() + "/.Trash") }
                        }
                    }
                }
            }
            Section {
                Button("Clear all saved states") { confirmClear = true }
                    .disabled(store.apps.allSatisfy { $0.state.isActive })
                LabeledContent("Location") {
                    HStack {
                        Text(verbatim: Format.tildePath(store.golden.info?.dir ?? "", home: NSHomeDirectory()))
                        Button("Location…") { if let dir = store.golden.info?.dir { SystemLinks.reveal(dir) } }
                    }
                }
                Text("To move it, remove the base image and build it in a new location.")
            }
        }
        .formStyle(.grouped)
        .confirmationDialog(Text("Clear the saved state of every app?"), isPresented: $confirmClear) {
            Button("Clear", role: .destructive) {
                Task {
                    for app in store.apps where !app.state.isActive {
                        await store.detail(for: app.name).clearSavedState()
                    }
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The next start of each app is cold, about 30 s. Your data in the apps stays.")
        }
    }
}

@MainActor
struct NetworkPane: View {
    @Environment(LauncherStore.self) private var store
    @AppStorage(SettingsKey.defaultNetwork, store: LauncherDefaults.store) private var defaultNetwork = NetworkMode.ask.rawValue
    @AppStorage(SettingsKey.panelPosition, store: LauncherDefaults.store) private var panelPosition = "window"

    var body: some View {
        Form {
            Section {
                Picker(selection: $defaultNetwork) {
                    ForEach(NetworkMode.allCases) { mode in
                        if mode == .ask {
                            Text("Ask (recommended)").tag(mode.rawValue)
                        } else {
                            Text(mode.title).tag(mode.rawValue)
                        }
                    }
                } label: {
                    Text("Default for new apps")
                }
                Picker("Panel position", selection: $panelPosition) {
                    Text("Next to the app's window").tag("window")
                    Text("Center of the screen").tag("center")
                }
                .onChange(of: panelPosition) { _, value in
                    Task { try? await store.service?.execute(PaddockArgs.panelPosition(value)) }
                }
                LabeledContent("Questions per minute before pausing") { Text(3, format: .number) }
            }
        }
        .formStyle(.grouped)
    }
}

@MainActor
struct AdvancedPane: View {
    @Environment(LauncherStore.self) private var store
    @AppStorage(SettingsKey.showCommands, store: LauncherDefaults.store) private var showCommands = false
    @State private var confirmReset = false
    @State private var runningDoctor = false

    var body: some View {
        Form {
            Section {
                Toggle(isOn: $showCommands) {
                    Text("Show the paddock command for each action")
                    Text("Adds a Terminal row under each sheet with the same command, for support.")
                }
            }
            Section {
                Button("Open Log Folder") {
                    SystemLinks.open(URL(fileURLWithPath: store.logDir ?? NSHomeDirectory() + "/Library/Logs/paddock"))
                }
                HStack {
                    Button("Run Doctor…") { runDoctor() }.disabled(runningDoctor || store.service == nil)
                    if runningDoctor { ProgressView().controlSize(.small).accessibilityLabel(Text("Running Doctor")) }
                }
                Text("Writes paddock's doctor report to a file and shows it in Finder. The report has no user name, paths or tokens: paddock removes them. It lists this Mac's model, macOS version, paddock version and the state of each app.")
            }
            Section {
                Button("Reset paddock for Mac settings") { confirmReset = true }
            }
        }
        .formStyle(.grouped)
        .confirmationDialog(Text("Reset the settings of paddock for Mac?"), isPresented: $confirmReset) {
            Button("Reset", role: .destructive) { LauncherDefaults.reset() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Your apps, their grants and the base image stay.")
        }
    }

    private func runDoctor() {
        guard let service = store.service else { return }
        runningDoctor = true
        Task {
            defer { runningDoctor = false }
            guard let report = try? await service.text(PaddockArgs.doctorReport) else { return }
            let folder = URL(fileURLWithPath: NSHomeDirectory() + "/Desktop")
            let file = folder.appendingPathComponent("paddock-doctor-\(Int(Date().timeIntervalSince1970)).txt")
            if (try? report.write(to: file, atomically: true, encoding: .utf8)) != nil {
                SystemLinks.reveal(file.path)
            }
        }
    }
}

/// `Uninstall…`: sized checkboxes, then a progress line, then the freed total.
@MainActor
struct UninstallSheet: View {
    @Environment(LauncherStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var plan = UninstallPlan()
    @State private var working = false
    @State private var freed: Int64?

    var body: some View {
        let sizes = UninstallPlan.sizes(apps: store.apps, golden: store.golden)
        VStack(alignment: .leading, spacing: 14) {
            Text("Uninstall paddock for Mac").font(.headline)
            if let freed {
                Text(UninstallCopy.done(freed)).accessibilityIdentifier("uninstall-done")
                HStack {
                    Spacer()
                    Button("Quit") { NSApp.terminate(nil) }.keyboardShortcut(.defaultAction)
                }
            } else {
                Toggle(isOn: $plan.removeApps) { Text(UninstallCopy.apps(sizes.apps)) }
                Toggle(isOn: $plan.removeGolden) { Text(UninstallCopy.golden(sizes.golden)) }
                Toggle(isOn: $plan.removeData) {
                    Text(UninstallCopy.data(sizes.data))
                    Text("Off: the data of each app goes to the Trash.")
                }
                .disabled(!plan.removeApps)
                Text("This removes everything paddock built. The paddock command from Homebrew stays; remove it with \"brew uninstall paddock\". The original apps in /Applications stay.")
                    .fixedSize(horizontal: false, vertical: true)
                CopyField(command: ShellCommands.brewUninstall)
                ForEach(plan.commands(apps: store.apps.map(\.name)), id: \.self) { args in
                    CommandEcho(args: args)
                }
                if working { ProgressView().progressViewStyle(.linear).accessibilityLabel(Text("Removing")) }
                HStack {
                    Spacer()
                    Button("Cancel", role: .cancel) { dismiss() }.keyboardShortcut(.cancelAction)
                    Button("Remove", role: .destructive) {
                        working = true
                        Task {
                            freed = await store.uninstall(plan)
                            working = false
                        }
                    }
                    .disabled(working || (!plan.removeApps && !plan.removeGolden))
                    .accessibilityIdentifier("uninstall-remove")
                }
            }
        }
        .padding(20)
        .frame(width: 520)
    }
}
