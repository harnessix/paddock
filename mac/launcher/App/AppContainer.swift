import AppKit
import Observation
import PaddockKit
import SwiftUI

/// The keys of the launcher's own settings.
enum SettingsKey {
    static let showInMenuBar = "showInMenuBar"
    static let keepRunning = "keepRunningOnQuit"
    static let notifyAfter = "notifyAfterSeconds"
    static let defaultNetwork = "defaultNetwork"
    static let panelPosition = "panelPosition"
    static let showCommands = "showCommands"
}

enum LauncherDefaults {
    /// Debug builds can use a separate defaults suite, so each UI test
    /// starts from clean settings.
    static let store: UserDefaults = {
        #if DEBUG
        if let suite = ProcessInfo.processInfo.environment["PADDOCK_LAUNCHER_DEFAULTS"],
           let defaults = UserDefaults(suiteName: suite) {
            return defaults
        }
        #endif
        return .standard
    }()

    static var keepRunning: Bool {
        #if DEBUG
        if let value = ProcessInfo.processInfo.environment["PADDOCK_LAUNCHER_KEEP_RUNNING"] { return value != "0" }
        #endif
        return store.object(forKey: SettingsKey.keepRunning) as? Bool ?? true
    }

    /// Seconds before a waiting question notifies; nil for never.
    static var notifyAfter: TimeInterval? {
        let value = store.object(forKey: SettingsKey.notifyAfter) as? Int ?? 30
        return value > 0 ? TimeInterval(value) : nil
    }

    static var defaultNetwork: NetworkMode {
        NetworkMode(rawValue: store.string(forKey: SettingsKey.defaultNetwork) ?? "ask") ?? .ask
    }

    /// `Reset paddock for Mac settings`.
    static func reset() {
        for key in [SettingsKey.showInMenuBar, SettingsKey.keepRunning, SettingsKey.notifyAfter,
                    SettingsKey.defaultNetwork, SettingsKey.panelPosition, SettingsKey.showCommands] {
            store.removeObject(forKey: key)
        }
    }
}

/// The sheets of the main window. Commands and the menu bar extra open them
/// through the router.
enum MainSheet: Identifiable, Equatable {
    case addApp(path: String?)
    case removeApp(String)
    case confirmStop(String)
    case clearState(String)
    case uninstall
    case removeGolden
    case rebuild
    case cancelBuild

    var id: String {
        switch self {
        case .addApp(let path): return "add:" + (path ?? "")
        case .removeApp(let name): return "remove:" + name
        case .confirmStop(let name): return "stop:" + name
        case .clearState(let name): return "clear:" + name
        case .uninstall: return "uninstall"
        case .removeGolden: return "removeGolden"
        case .rebuild: return "rebuild"
        case .cancelBuild: return "cancelBuild"
        }
    }
}

@MainActor
@Observable
final class UIRouter {
    var sheet: MainSheet?
    /// Set once the onboarding finished: it then opens the Add App sheet.
    var addAfterOnboarding = false
}

/// The one store, router and window opener of the process. The App scenes
/// and the AppKit delegate share them.
@MainActor
enum AppContainer {
    static let store = LauncherStore(service: nil)
    static let router = UIRouter()
    /// Captured from a SwiftUI view, so AppKit code can open windows.
    static var openWindow: OpenWindowAction?
    /// The one first load. The delegate and the main window both await it,
    /// so the version alert never checks a binary that is still `checking`.
    private static var startTask: Task<Void, Never>?

    static var isDebugBuild: Bool {
        #if DEBUG
        return true
        #else
        return false
        #endif
    }

    static var isUITest: Bool {
        ProcessInfo.processInfo.environment["PADDOCK_LAUNCHER_UITEST"] == "1"
    }

    static func locator() -> BinaryLocator {
        BinaryLocator(
            environment: ProcessInfo.processInfo.environment,
            home: NSHomeDirectory(),
            isDebugBuild: isDebugBuild,
            isExecutable: { FileManager.default.isExecutableFile(atPath: $0) },
            brewPrefix: { brew in
                let process = Process()
                process.executableURL = URL(fileURLWithPath: brew)
                process.arguments = ["--prefix"]
                let pipe = Pipe()
                process.standardOutput = pipe
                process.standardError = FileHandle.nullDevice
                guard (try? process.run()) != nil else { return nil }
                process.waitUntilExit()
                guard process.terminationStatus == 0 else { return nil }
                return String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            })
    }

    /// Finds paddock and loads the state once per launch.
    static func start() async {
        let task = startTask ?? Task { await checkAgain() }
        startTask = task
        await task.value
    }

    /// `Check Again`: looks for paddock again and reloads.
    static func checkAgain() async {
        let locator = locator()
        await store.start(locate: { locator.locate() },
                          makeService: { CLIPaddockService(runner: ProcessRunner(executable: $0)) })
    }

    static func open(_ id: String) {
        openWindow?(id: id)
        NSApp.activate()
    }

    static func showMain(_ selection: SidebarItem? = nil) {
        if let selection { store.selection = selection }
        open(WindowID.main)
    }
}

enum WindowID {
    static let main = "main"
    static let onboarding = "onboarding"
}
