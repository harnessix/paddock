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
                SessionHeader(app: app)
                    .padding()
                notices(app)
            }
            TabView(selection: $store.detailTab) {
                OverviewTab(model: model)
                    .tabItem { Text("Overview") }
                    .tag(DetailTab.overview)
                    .accessibilityHidden(store.detailTab != .overview)
                GrantsView(model: model)
                    .tabItem { Text("Grants") }
                    .tag(DetailTab.grants)
                    .accessibilityHidden(store.detailTab != .grants)
                LogView(model: model)
                    .tabItem { Text("Log") }
                    .tag(DetailTab.log)
                    .accessibilityHidden(store.detailTab != .log)
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
                ErrorShape(error: .appChanged(app: app.title))
            }
            if let line = app.json.copyFailed {
                ErrorShape(error: .appCopyFailed(app: app.title, line: line), primary: { store.detailTab = .log })
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

@MainActor
struct OverviewTab: View {
    let model: AppDetailModel
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router

    var body: some View {
        if let app = store.app(named: model.name) {
            Form {
                InstancesSection(app: app)
                Section("Status") {
                    LabeledContent {
                        Text(StatusPresentation.of(app.state, now: Date()).text).labelInk()
                            .accessibilityIdentifier("overview-state")
                    } label: { Text("State").labelInk() }
                    if let since = app.json.since {
                        LabeledContent {
                            Text(since, format: .dateTime.day().month().hour().minute()).labelInk()
                        } label: { Text("Since").labelInk() }
                    }
                    if let memory = OverviewCopy.memory(app.json) {
                        LabeledContent { Text(memory).labelInk() } label: { Text("Memory").labelInk() }
                    }
                    if let golden = app.json.golden {
                        LabeledContent { Text(verbatim: golden).labelInk() } label: { Text("Base image").labelInk() }
                    }
                    if let next = app.json.nextStart {
                        LabeledContent { Text(verbatim: next).labelInk() } label: { Text("Next start").labelInk() }
                    }
                    if app.json.sizes == nil, let bytes = app.json.bytes, bytes > 0 {
                        LabeledContent { Text(verbatim: Format.gb(bytes)).labelInk() } label: { Text("On disk").labelInk() }
                    }
                }
                if let sizes = app.json.sizes {
                    Section("Disk") {
                        LabeledContent { Text(verbatim: Format.gb(sizes.appCopy)).labelInk() } label: { Text("App copy").labelInk() }
                        LabeledContent { Text(verbatim: Format.gb(sizes.data)).labelInk() } label: { Text("Data").labelInk() }
                        LabeledContent { Text(verbatim: Format.gb(sizes.savedState)).labelInk() } label: { Text("Saved state").labelInk() }
                        LabeledContent { Text(verbatim: Format.gb(sizes.total)).labelInk() } label: { Text("Total").labelInk() }
                        Button("Clear saved state") { router.sheet = .clearState(app.key) }
                            .disabled(sizes.savedState == 0 || app.state.holdsSession)
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
                    .menuAuditAction()
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
                        Button("Remove App…", role: .destructive) { router.sheet = .removeApp(app.key) }
                            .accessibilityIdentifier("remove-app-button")
                    }
                }
            }
            .formStyle(.grouped)
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text("Overview"))
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
                        .labelInk()
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
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Log"))
        .task { await model.loadLog() }
        .onDisappear { model.setFollow(false) }
    }
}
