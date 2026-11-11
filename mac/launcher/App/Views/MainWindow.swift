import AppKit
import PaddockKit
import SwiftUI
import UniformTypeIdentifiers

/// The main window: a sidebar (Apps, Questions, Base image) and a detail.
/// The sidebar is the only navigation. An app dropped on the window, or on
/// the Dock icon, opens the review sheet.
@MainActor
struct MainWindow: View {
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow
    @State private var dropTargeted = false

    var body: some View {
        @Bindable var router = router
        NavigationSplitView {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 230)
                .accessibilityElement(children: .contain)
                .accessibilityLabel(Text("Sidebar"))
        } detail: {
            DetailRouter()
                .accessibilityElement(children: .contain)
                .accessibilityLabel(Text("Details"))
        }
        .toolbarBackground(Color(nsColor: .windowBackgroundColor), for: .windowToolbar)
        .toolbarBackground(.visible, for: .windowToolbar)
        .background(Color(nsColor: .windowBackgroundColor).accessibilityHidden(true))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Main window"))
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                StreamStatusLine()
                ToastBar(toast: store.toast)
            }
        }
        .overlay { DropHighlight(active: dropTargeted) }
        .dropDestination(for: URL.self) { urls, _ in
            guard store.binary.isUsable else { return false }
            AddCenter.shared.receive(urls.filter(\.isFileURL).map(\.path))
            return true
        } isTargeted: { dropTargeted = $0 && store.binary.isUsable }
        .sheet(item: $router.sheet) { sheet in
            MainSheetView(sheet: sheet)
                .environment(store)
                .environment(router)
        }
        .onAppear {
            AppContainer.openWindow = openWindow
        }
        .task {
            await AppContainer.start()
            guard LaunchOnce.claim() else { return }
            SelfInstaller.offerIfNeeded()
            #if DEBUG
            // UI tests stand in for a drop: the same path as a real one.
            if let drop = ProcessInfo.processInfo.environment["PADDOCK_LAUNCHER_DROP"], store.binary.isUsable {
                AddCenter.shared.receive(drop.split(separator: ":").map(String.init))
            }
            #endif
        }
        .onChange(of: store.needsOnboarding, initial: true) { _, needs in
            if needs {
                openWindow(id: WindowID.onboarding)
                dismissWindow(id: WindowID.main)
            }
        }
        .onChange(of: store.apps.isEmpty, initial: true) { _, _ in
            if store.selection == nil, let first = store.apps.first { store.selection = .app(first.key) }
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
            Text("Apps")
                .font(.subheadline)
                .labelInk()
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityElement(children: .combine)
                .accessibilityAddTraits(.isHeader)
                .accessibilityLabel(Text("Apps"))
                .selectionDisabled()
            LibrarySidebarRows()
            // A list row's static text is not hittable. A plain button is,
            // and it has the press action the audit requires.
            Button {
                store.selection = .questions
            } label: {
                Label { Text("Questions").labelInk() } icon: { DecorativeSymbol(name: "questionmark.bubble") }
                    .badge(store.questions.count)
            }
            .buttonStyle(.plain)
            .labelInk()
            .tag(SidebarItem.questions)
            .accessibilityLabel(Text("Questions"))
            .accessibilityIdentifier("sidebar-questions")
            Button {
                store.selection = .baseImage
            } label: {
                Label { Text("Base image").labelInk() } icon: { DecorativeSymbol(name: "externaldrive") }
            }
            .buttonStyle(.plain)
            .labelInk()
            .tag(SidebarItem.baseImage)
            .accessibilityLabel(Text("Base image"))
            .accessibilityIdentifier("sidebar-base-image")
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .background(Color(nsColor: .windowBackgroundColor))
        .accessibilityIdentifier("sidebar")
        .id(store.apps.map(\.key).joined(separator: "|"))
        .onDeleteCommand {
            if let app = store.selectedApp { router.sheet = .removeApp(app.key) }
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
                    Text(verbatim: app.title).labelInk()
                    // Label color: secondary and vibrant primary fail the contrast audit.
                    Text(presentation.text)
                        .font(.caption)
                        .labelInk()
                        .lineLimit(1)
                        .accessibilityIdentifier("status-text-" + app.key)
                }
                Spacer(minLength: 4)
                AppStatusView(presentation: presentation)
                    .accessibilityIdentifier("status-symbol-" + app.key)
            }
            // `.contain` keeps the status text and symbol as children with
            // their identifiers; the row still speaks name, text and state.
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text(verbatim: [app.title, presentation.text.localized,
                                                presentation.accessibilityLabel.localized].joined(separator: ", ")))
            .accessibilityIdentifier("app-row-" + app.key)
        }
    }
}

@MainActor
struct AppRowMenu: View {
    let app: AppItem
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router

    var body: some View {
        ForEach(SessionButtons.of(app.state, canSuspend: store.capabilities.suspend), id: \.self) { button in
            Button(SessionMenuTitle.of(button)) { SessionCommand.perform(button, app, store: store, router: router) }
        }
        Button("Open New Instance") { SessionCommand.newInstance(app, store: store) }
            .disabled(app.state.isBlocked)
        Divider()
        Button("Remove App…") { router.sheet = .removeApp(app.key) }
    }
}

/// The detail for the sidebar selection.
@MainActor
struct DetailRouter: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        if store.binary == .checking || (store.binary.isUsable && !store.listLoaded && store.listError == nil) {
            LibraryStateView(kind: .loading)
        } else if !store.binary.isUsable, store.apps.isEmpty {
            LibraryStateView(kind: .offline)
        } else if let line = store.listError, store.apps.isEmpty {
            LibraryStateView(kind: .failed(line))
        } else {
            selected
        }
    }

    @ViewBuilder private var selected: some View {
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
                PlainEmpty {
                    Label { Text("Select an app") } icon: { DecorativeSymbol(name: "sidebar.left") }
                } description: {
                    EmptyView()
                } actions: {
                    EmptyView()
                }
                .accessibilityElement(children: .contain)
                .accessibilityLabel(Text("Select an app"))
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
        PlainEmpty {
            Label { Text("No apps yet") } icon: { DecorativeSymbol(name: "app.dashed") }
        } description: {
            Text("Add an app from your Applications folder. It runs in its own sandbox the next time you start it.")
        } actions: {
            Button("Add App…") { router.sheet = .addApp(path: nil) }
                .keyboardShortcut(.defaultAction)
                .disabled(!store.binary.isUsable)
                .accessibilityIdentifier("empty-add-app")
            Text(LibraryCopy.dropHint).font(.callout).labelInk()
        }
        // Without `.contain` the identifier goes to every child and hides
        // `empty-add-app`.
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("No apps yet"))
        .accessibilityIdentifier("empty-state")
    }
}

/// The sheet for the router's current sheet.
@MainActor
struct MainSheetView: View {
    let sheet: MainSheet

    var body: some View {
        switch sheet {
        case .addApp(let path): AddReviewSheet(initialPath: path)
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

/// The menu titles of the session buttons.
enum SessionMenuTitle {
    static func of(_ button: SessionButtons) -> LocalizedStringKey {
        switch button {
        case .start: return "Start" // localized
        case .resume: return "Resume" // localized
        case .open: return "Open" // localized
        case .suspend: return "Suspend" // localized
        case .stop: return "Stop" // localized
        }
    }
}
