import PaddockKit
import SwiftUI
import UniformTypeIdentifiers

/// The main window: a sidebar (Apps, Questions, Base image) and a detail.
/// The sidebar is the only navigation.
@MainActor
struct MainWindow: View {
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        @Bindable var router = router
        NavigationSplitView {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 230)
        } detail: {
            DetailRouter()
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                StreamStatusLine()
                ToastBar(toast: store.toast)
            }
        }
        .dropDestination(for: URL.self) { urls, _ in
            guard store.binary.isUsable, let app = urls.first(where: { $0.pathExtension == "app" }) else { return false }
            router.sheet = .addApp(path: app.path)
            return true
        }
        .sheet(item: $router.sheet) { sheet in
            MainSheetView(sheet: sheet)
                .environment(store)
                .environment(router)
        }
        .onAppear { AppContainer.openWindow = openWindow }
        .task { await AppContainer.start() }
        .onChange(of: store.needsOnboarding, initial: true) { _, needs in
            if needs {
                openWindow(id: WindowID.onboarding)
                dismissWindow(id: WindowID.main)
            }
        }
        .onChange(of: store.apps.isEmpty, initial: true) { _, _ in
            if store.selection == nil, let first = store.apps.first { store.selection = .app(first.name) }
        }
    }
}

@MainActor
struct Sidebar: View {
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router

    var body: some View {
        @Bindable var store = store
        List(selection: $store.selection) {
            Section("Apps") {
                ForEach(store.apps) { app in
                    AppRow(app: app)
                        .tag(SidebarItem.app(app.name))
                        .contextMenu { AppRowMenu(app: app) }
                }
            }
            Section {
                Label { Text("Questions") } icon: { DecorativeSymbol(name: "questionmark.bubble") }
                    .badge(store.questions.count)
                    .tag(SidebarItem.questions)
                    .accessibilityIdentifier("sidebar-questions")
                Label { Text("Base image") } icon: { DecorativeSymbol(name: "externaldrive") }
                    .tag(SidebarItem.baseImage)
                    .accessibilityIdentifier("sidebar-base-image")
            }
        }
        .listStyle(.sidebar)
        .accessibilityIdentifier("sidebar")
        .onDeleteCommand {
            if let app = store.selectedApp { router.sheet = .removeApp(app.name) }
        }
        .safeAreaInset(edge: .bottom) {
            HStack {
                Button {
                    router.sheet = .addApp(path: nil)
                } label: {
                    Image(systemName: "plus")
                }
                .buttonStyle(.borderless)
                .help(Text("Add App…"))
                .accessibilityLabel(Text("Add App…"))
                .accessibilityIdentifier("add-app-button")
                .disabled(!store.binary.isUsable)
                Spacer()
            }
            .padding(8)
        }
    }
}

/// A sidebar row: icon, name, status text, status symbol.
@MainActor
struct AppRow: View {
    let app: AppItem

    var body: some View {
        TimelineView(.periodic(from: .now, by: 60)) { context in
            let presentation = StatusPresentation.of(app.state, now: context.date)
            HStack(spacing: 8) {
                AppIcon(path: app.json.bundlePath)
                VStack(alignment: .leading, spacing: 1) {
                    Text(verbatim: app.name)
                    // Label color: secondaryLabelColor is under 4.5:1 in Light.
                    Text(presentation.text)
                        .font(.caption)
                        .lineLimit(1)
                        .accessibilityIdentifier("status-text-" + app.name)
                }
                Spacer(minLength: 4)
                AppStatusView(presentation: presentation)
                    .accessibilityIdentifier("status-symbol-" + app.name)
            }
            // `.contain` keeps the status text and symbol as children with
            // their identifiers; the row still speaks name, text and state.
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text(verbatim: [app.name, presentation.text.localized,
                                                presentation.accessibilityLabel.localized].joined(separator: ", ")))
            .accessibilityIdentifier("app-row-" + app.name)
        }
    }
}

@MainActor
struct AppRowMenu: View {
    let app: AppItem
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router

    var body: some View {
        if app.state.canStart {
            Button("Start") { Task { await store.startApp(app.name) } }
        }
        if app.state.canStop {
            Button("Stop") { Task { await store.stopApp(app.name) } }
        }
        Divider()
        Button("Remove App…") { router.sheet = .removeApp(app.name) }
    }
}

/// The detail for the sidebar selection.
@MainActor
struct DetailRouter: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        switch store.selection {
        case .app(let name)?:
            if store.app(named: name) != nil {
                AppDetailView(name: name)
                    .id(name)
            } else {
                EmptyAppsView()
            }
        case .questions?:
            QuestionsView()
        case .baseImage?:
            BaseImageView()
        case nil:
            if store.apps.isEmpty {
                EmptyAppsView()
            } else {
                ContentUnavailableView {
                    Label { Text("Select an app") } icon: { DecorativeSymbol(name: "sidebar.left") }
                }
            }
        }
    }
}

/// No apps yet.
@MainActor
struct EmptyAppsView: View {
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router

    var body: some View {
        ContentUnavailableView {
            Label { Text("No apps yet") } icon: { DecorativeSymbol(name: "app.dashed") }
        } description: {
            Text("Add an app from your Applications folder. It runs in its own sandbox the next time you start it.")
        } actions: {
            Button("Add App…") { router.sheet = .addApp(path: nil) }
                .keyboardShortcut(.defaultAction)
                .disabled(!store.binary.isUsable)
                .accessibilityIdentifier("empty-add-app")
        }
        // Without `.contain` the identifier goes to every child and hides
        // `empty-add-app`.
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("empty-state")
    }
}

/// The sheet for the router's current sheet.
@MainActor
struct MainSheetView: View {
    let sheet: MainSheet

    var body: some View {
        switch sheet {
        case .addApp(let path): AddAppSheet(initialPath: path)
        case .removeApp(let name): RemoveAppSheet(name: name)
        case .confirmStop(let name): ConfirmStopSheet(name: name)
        case .clearState(let name): ClearStateSheet(name: name)
        case .uninstall: UninstallSheet()
        case .removeGolden: RemoveGoldenSheet()
        case .rebuild: RebuildSheet()
        case .cancelBuild: CancelBuildSheet()
        }
    }
}
