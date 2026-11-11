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

    /// One row of an outline. `List(_:children:)` is an NSOutlineView, which
    /// is what the UI tests query with `app.outlines`.
    private struct Node: Identifiable {
        let pane: Pane
        var id: Pane { pane }
        var children: [Node]? { nil }
    }

    private var nodes: [Node] { Pane.allCases.map { Node(pane: $0) } }

    var body: some View {
        NavigationSplitView {
            List(nodes, children: \.children, selection: $pane) { node in
                Button {
                    pane = node.pane
                } label: {
                    paneLabel(node.pane)
                }
                .buttonStyle(.plain)
                .tag(node.pane)
                .accessibilityIdentifier("settings-" + node.pane.rawValue)
            }
            .listStyle(.sidebar)
            .scrollContentBackground(.hidden)
            .background(Color(nsColor: .windowBackgroundColor))
            .navigationSplitViewColumnWidth(170)
        } detail: {
            switch pane ?? .general {
            case .general: GeneralPane()
            case .storage: StorageView()
            case .network: NetworkPane()
            case .advanced: AdvancedPane()
            }
        }
        .accessibilityIdentifier("settings-root")
        .background(Color(nsColor: .windowBackgroundColor))
    }

    @ViewBuilder
    private func paneLabel(_ pane: Pane) -> some View {
        switch pane {
        case .general:
            Label { Text("General").labelInk() } icon: { DecorativeSymbol(name: "gearshape") }
                .accessibilityLabel(Text("General"))
        case .storage:
            Label { Text("Storage").labelInk() } icon: { DecorativeSymbol(name: "internaldrive") }
                .accessibilityLabel(Text("Storage"))
        case .network:
            Label { Text("Network").labelInk() } icon: { DecorativeSymbol(name: "network") }
                .accessibilityLabel(Text("Network"))
        case .advanced:
            Label { Text("Advanced").labelInk() } icon: { DecorativeSymbol(name: "wrench.and.screwdriver") }
                .accessibilityLabel(Text("Advanced"))
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
                    Text("When this is off, open paddock for Mac from the Applications folder to get the window and this setting back.").labelInk()
                }
                Toggle(isOn: $keepRunning) {
                    Text("Keep running apps when paddock for Mac quits")
                    Text("When this is off, quitting stops each app and saves its state.").labelInk()
                }
                Picker("Notify when a question waits longer than", selection: $notifyAfter) {
                    Text("30 s").tag(30)
                    Text("2 min").tag(120)
                    Text("Never").tag(0)
                }
                .menuAuditAction()
            }
            Section {
                LabeledContent("Language") {
                    Text("paddock for Mac uses the language of macOS.")
                }
            }
            AboutSection()
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
                .menuAuditAction()
                Picker("Panel position", selection: $panelPosition) {
                    Text("Next to the app's window").tag("window")
                    Text("Center of the screen").tag("center")
                }
                .menuAuditAction()
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
                    Text("Adds a Terminal row under each sheet with the same command, for support.").labelInk()
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
                    Text("Off: the data of each app goes to the Trash.").labelInk()
                }
                .disabled(!plan.removeApps)
                Text("This removes everything paddock built. The paddock command from Homebrew stays; remove it with \"brew uninstall paddock\". The original apps in /Applications stay.")
                    .labelInk()
                    .fixedSize(horizontal: false, vertical: true)
                CopyField(command: ShellCommands.brewUninstall)
                ForEach(plan.commands(apps: Array(Set(store.apps.map(\.instance.app))).sorted()), id: \.self) { args in
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
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Uninstall paddock for Mac"))
    }
}

/// The versions of paddock for Mac and paddock, and where the launcher
/// runs from, with the move into ~/Applications.
@MainActor
struct AboutSection: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        let place = SelfInstaller.place
        Section {
            LabeledContent("Version") {
                Text(LibraryCopy.versions(launcher: SelfInstaller.version, paddock: paddockVersion))
                    .labelInk()
                    .textSelection(.enabled)
                    .accessibilityIdentifier("about-versions")
            }
            LabeledContent("paddock needed") {
                Text(verbatim: VersionGate.current.minimumText + "+")
            }
            LabeledContent("Location") {
                HStack {
                    Text(SelfInstall.locationLine(place))
                    if SelfInstall.shouldOffer(place, declined: false) {
                        Button("Move to Applications…") { SelfInstaller.move() }
                            .accessibilityIdentifier("about-move")
                    }
                }
            }
        } header: {
            Text("About")
        }
    }

    private var paddockVersion: String? {
        if case .found(_, let verdict) = store.binary {
            switch verdict {
            case .ok(let version), .tooNew(let version): return version
            case .tooOld(let have, _): return have
            case .unreadable: return nil
            }
        }
        return nil
    }
}
