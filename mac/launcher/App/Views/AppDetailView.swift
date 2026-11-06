import PaddockKit
import SwiftUI

/// The detail of one app: a header, in-place errors, and the tabs
/// Overview, Grants and Log.
@MainActor
struct AppDetailView: View {
    let name: String
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router

    var body: some View {
        @Bindable var store = store
        let model = store.detail(for: name)
        VStack(spacing: 0) {
            if let app = store.app(named: name) {
                AppHeader(app: app)
                    .padding()
                notices(app)
            }
            TabView(selection: $store.detailTab) {
                OverviewTab(model: model)
                    .tabItem { Text("Overview") }
                    .tag(DetailTab.overview)
                GrantsView(model: model)
                    .tabItem { Text("Grants") }
                    .tag(DetailTab.grants)
                LogView(model: model)
                    .tabItem { Text("Log") }
                    .tag(DetailTab.log)
            }
            .padding([.horizontal, .bottom])
        }
        .task(id: name) { await model.load(force: true) }
    }

    @ViewBuilder
    private func notices(_ app: AppItem) -> some View {
        VStack(spacing: 8) {
            if let error = store.appErrors[name] {
                ErrorShape(error: error,
                           primary: { primary(for: error) },
                           showLog: { store.detailTab = .log },
                           stopChoices: stopChoices(error),
                           stop: { other in Task { await store.stopApp(other) } })
            }
            if app.json.agentConnected == false, app.state.isRunning {
                ErrorShape(error: .agentAway)
            }
            if app.json.appChanged == true {
                ErrorShape(error: .appChanged(app: name))
            }
            if let line = app.json.copyFailed {
                ErrorShape(error: .appCopyFailed(app: name, line: line), primary: { store.detailTab = .log })
            }
            if case .blocked(_, "no_golden"?) = app.state {
                ErrorShape(error: .goldenRemoved, primary: { store.selection = .baseImage })
            }
        }
        .padding(.horizontal)
    }

    private func stopChoices(_ error: LauncherError) -> [String] {
        if case .twoRunning(_, let running) = error { return running }
        return []
    }

    private func primary(for error: LauncherError) {
        switch error.primary {
        case .manageStorage: SystemLinks.manageStorage()
        case .startAgain: Task { await store.startApp(name) }
        case .showLog: store.detailTab = .log
        case .build: store.selection = .baseImage
        default: break
        }
    }
}

/// Icon, name, bundle path, and Start / Open / Stop.
@MainActor
struct AppHeader: View {
    let app: AppItem
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router

    var body: some View {
        HStack(spacing: 12) {
            AppIcon(path: app.json.bundlePath, size: 48)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: app.name).font(.title2)
                if let path = app.json.bundlePath {
                    Text(verbatim: path).textSelection(.enabled)
                }
            }
            Spacer()
            if app.state.canStop {
                Button("Stop") { stop() }
                    .accessibilityIdentifier("stop-button")
            }
            if app.state.isRunning {
                Button("Open") { Task { await store.openApp(app.name) } }
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("open-button")
            } else {
                Button("Start") { Task { await store.startApp(app.name) } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!app.state.canStart)
                    .accessibilityIdentifier("start-button")
            }
        }
    }

    /// No sheet in the normal case: only with open questions.
    private func stop() {
        if store.questions.contains(where: { $0.app == app.name }) {
            router.sheet = .confirmStop(app.name)
        } else {
            Task { await store.stopApp(app.name) }
        }
    }
}

@MainActor
struct OverviewTab: View {
    let model: AppDetailModel
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router

    var body: some View {
        if let app = store.app(named: model.name) {
            Form {
                Section("Status") {
                    LabeledContent("State") {
                        Text(StatusPresentation.of(app.state, now: Date()).text)
                            .accessibilityIdentifier("overview-state")
                    }
                    if let since = app.json.since {
                        LabeledContent("Since") { Text(since, format: .dateTime.day().month().hour().minute()) }
                    }
                    if let memory = OverviewCopy.memory(app.json) {
                        LabeledContent("Memory") { Text(memory) }
                    }
                    if let golden = app.json.golden {
                        LabeledContent("Base image") { Text(verbatim: golden) }
                    }
                    if let next = app.json.nextStart {
                        LabeledContent("Next start") { Text(verbatim: next) }
                    }
                }
                if let sizes = app.json.sizes {
                    Section("Disk") {
                        LabeledContent("App copy") { Text(verbatim: Format.gb(sizes.appCopy)) }
                        LabeledContent("Data") { Text(verbatim: Format.gb(sizes.data)) }
                        LabeledContent("Saved state") { Text(verbatim: Format.gb(sizes.savedState)) }
                        LabeledContent("Total") { Text(verbatim: Format.gb(sizes.total)) }
                        Button("Clear saved state") { router.sheet = .clearState(app.name) }
                            .disabled(sizes.savedState == 0 || app.state.isActive)
                    }
                }
                Section("Network") {
                    Picker("Network", selection: Binding(
                        get: { model.networkMode },
                        set: { mode in Task { await model.setNetwork(mode) } })) {
                        ForEach(NetworkMode.allCases) { mode in
                            Text(mode.title).tag(mode)
                        }
                    }
                    Button { store.selection = .questions } label: {
                        Text(OverviewCopy.openQuestions(store.questions.filter { $0.app == app.name }.count))
                    }
                    .buttonStyle(.link)
                    Button { store.detailTab = .grants } label: {
                        Text(OverviewCopy.rules(model.rules.isEmpty ? (app.json.ruleCount ?? 0) : model.rules.count))
                    }
                    .buttonStyle(.link)
                }
                Section {
                    HStack {
                        Button("Show Log") { store.detailTab = .log }
                        Spacer()
                        Button("Remove App…", role: .destructive) { router.sheet = .removeApp(app.name) }
                            .accessibilityIdentifier("remove-app-button")
                    }
                }
            }
            .formStyle(.grouped)
        }
    }
}

/// `paddock app log NAME --events`, as paddock prints it.
@MainActor
struct LogView: View {
    let model: AppDetailModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Toggle("Follow", isOn: Binding(get: { model.following }, set: { model.setFollow($0) }))
                Spacer()
                Button("Copy") { Pasteboard.copy(model.logLines.joined(separator: "\n")) }
                Button("Reveal in Finder") {
                    if let path = model.status?.logPath { SystemLinks.reveal(path) }
                }
                .disabled(model.status?.logPath == nil)
            }
            ScrollViewReader { proxy in
                ScrollView {
                    Text(verbatim: model.logLines.joined(separator: "\n"))
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                    Color.clear.frame(height: 1).id("end")
                }
                .accessibilityLabel(Text("Log"))
                .accessibilityIdentifier("log-text")
                .onChange(of: model.logLines.count) { _, _ in
                    if model.following { proxy.scrollTo("end") }
                }
            }
            .background(.background)
            .border(.separator)
        }
        .padding(.top, 8)
        .task { await model.loadLog() }
        .onDisappear { model.setFollow(false) }
    }
}
