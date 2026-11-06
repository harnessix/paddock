import PaddockKit
import SwiftUI

/// The menu of the status item: a control surface for running apps. The
/// window is for setup.
@MainActor
struct MenuBarContent: View {
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router
    @Environment(\.openWindow) private var openWindow
    @AppStorage(SettingsKey.keepRunning, store: LauncherDefaults.store) private var keepRunning = true

    var body: some View {
        let menu = MenuModel.of(store, keepRunning: keepRunning)
        Text(menu.header)
        Divider()
        ForEach(menu.apps) { app in
            appMenu(app)
        }
        if menu.showsMore {
            Button("More…") { showMain(nil) }
        }
        if let questions = menu.questionsItem {
            Divider()
            Button { showMain(.questions) } label: { Text(questions) }
        }
        Divider()
        Button("Open paddock") { showMain(nil) }
            .keyboardShortcut("o")
        SettingsLink { Text("Settings…") }
            .keyboardShortcut(",")
        Divider()
        Button("Quit paddock for Mac") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
        if let note = menu.quitNote {
            Text(note)
        }
    }

    private func appMenu(_ app: AppItem) -> some View {
        let presentation = StatusPresentation.of(app.state, now: Date())
        return Menu {
            if app.state.isRunning {
                Button("Open") { Task { await store.openApp(app.name) } }
            }
            if app.state.canStart {
                Button("Start") { Task { await store.startApp(app.name) } }
            }
            if app.state.canStop {
                Button("Stop") { Task { await store.stopApp(app.name) } }
            }
            Divider()
            Button("Grants…") {
                store.detailTab = .grants
                showMain(.app(app.name))
            }
            Button("Show Log") {
                store.detailTab = .log
                showMain(.app(app.name))
            }
        } label: {
            Label {
                Text(verbatim: app.name)
            } icon: {
                Image(systemName: presentation.symbol ?? "circle.dotted")
            }
            .accessibilityLabel(Text(verbatim: app.name + ", " + presentation.accessibilityLabel.localized))
        } primaryAction: {
            Task { await store.primaryAction(app.name) }
        }
    }

    private func showMain(_ selection: SidebarItem?) {
        if let selection { store.selection = selection }
        openWindow(id: WindowID.main)
        NSApp.activate()
    }
}
