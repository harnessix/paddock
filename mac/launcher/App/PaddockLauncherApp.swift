import AppKit
import PaddockKit
import SwiftUI

/// paddock for Mac: one process with a menu bar extra, one main window,
/// the onboarding window and the Settings window. SwiftUI views run inside
/// an AppKit lifecycle.
@main
struct PaddockLauncherApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @AppStorage(SettingsKey.showInMenuBar, store: LauncherDefaults.store) private var showInMenuBar = true

    private var store: LauncherStore { AppContainer.store }
    private var router: UIRouter { AppContainer.router }

    var body: some Scene {
        Window("paddock", id: WindowID.main) {
            MainWindow()
                .environment(store)
                .environment(router)
                .frame(minWidth: 760, minHeight: 480)
        }
        .defaultSize(width: 920, height: 620)
        .commands { LauncherCommands(store: store, router: router) }

        Window("Welcome to paddock", id: WindowID.onboarding) {
            OnboardingView()
                .environment(store)
                .environment(router)
                .frame(width: 640, height: 480)
        }
        .windowResizability(.contentSize)
        .defaultPosition(.center)

        MenuBarExtra(isInserted: $showInMenuBar) {
            MenuBarContent()
                .environment(store)
                .environment(router)
        } label: {
            MenuBarLabel()
                .environment(store)
        }
        .menuBarExtraStyle(.menu)

        Settings {
            SettingsView()
                .environment(store)
                .frame(minWidth: 640, minHeight: 560)
        }
    }
}

/// The menu bar commands. Every action is also in the window; these are
/// the keyboard paths.
@MainActor
struct LauncherCommands: Commands {
    let store: LauncherStore
    let router: UIRouter
    @Environment(\.openWindow) private var openWindow

    var body: some Commands {
        CommandGroup(after: .appSettings) {
            Button("Uninstall…") {
                openWindow(id: WindowID.main)
                router.sheet = .uninstall
            }
        }
        CommandGroup(replacing: .newItem) {
            Button("Add App…") {
                openWindow(id: WindowID.main)
                router.sheet = .addApp(path: nil)
            }
            .keyboardShortcut("n")
            .disabled(!store.binary.isUsable)
        }
        CommandGroup(after: .sidebar) {
            Button("Apps") {
                store.selection = store.apps.first.map { .app($0.key) }
            }
            .keyboardShortcut("1")
            Button("Questions") { store.selection = .questions }
                .keyboardShortcut("2")
            Button("Base image") { store.selection = .baseImage }
                .keyboardShortcut("3")
            Divider()
            Button("Refresh") { Task { await store.refresh() } }
                .keyboardShortcut("r")
        }
        CommandMenu("App") {
            Button(startStopTitle) { Task { await store.toggleSelected() } }
                .keyboardShortcut("s", modifiers: [.command, .shift])
                .disabled(store.selectedApp == nil)
            Button("Show Log") { store.detailTab = .log }
                .keyboardShortcut("l")
                .disabled(store.selectedApp == nil)
        }
    }

    private var startStopTitle: LocalizedStringKey {
        store.selectedApp?.state.canStop == true ? "Stop" : "Start" // localized
    }
}

/// The status item icon. A template image: monochrome, the system tints it.
@MainActor
struct MenuBarLabel: View {
    @Environment(LauncherStore.self) private var store
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Image(nsImage: MenuBarIcon.image(for: MenuModel.of(store, keepRunning: LauncherDefaults.keepRunning).iconState))
            .accessibilityLabel(Text("paddock"))
            .onAppear { AppContainer.openWindow = openWindow }
    }
}

enum MenuBarIcon {
    static func symbol(for state: MenuModel.IconState) -> String {
        switch state {
        case .idle: return "shippingbox"
        case .running: return "shippingbox.fill"
        case .question: return "exclamationmark.shield.fill"
        }
    }

    static func image(for state: MenuModel.IconState) -> NSImage {
        let image = NSImage(systemSymbolName: symbol(for: state), accessibilityDescription: "paddock") ?? NSImage()
        image.isTemplate = true
        return image
    }
}
