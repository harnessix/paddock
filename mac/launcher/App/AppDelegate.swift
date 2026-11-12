import AppKit
import PaddockKit
import SwiftUI
import UserNotifications

/// The AppKit side: launch, quit, app-level alerts and notifications.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    private var notified: Set<String> = []
    private var notifyTimer: Timer?
    private var quitting = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        #if DEBUG
        SnapshotDump.scheduleIfAsked()
        #endif
        if !AppContainer.isUITest {
            let center = UNUserNotificationCenter.current()
            center.delegate = self
        }
        Task {
            await AppContainer.start()
            showVersionAlertIfNeeded()
        }
        // A local check of the questions already loaded; it never calls paddock.
        notifyTimer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.notifyWaitingQuestions() }
        }
    }

    /// An app dropped on the Dock icon, or opened with paddock for Mac from
    /// Finder: the same review sheet as a drop on the window.
    func application(_ application: NSApplication, open urls: [URL]) {
        let paths = urls.filter(\.isFileURL).map(\.path)
        Task {
            await AppContainer.start()
            guard AppContainer.store.binary.isUsable else {
                AppContainer.showMain(nil)
                return
            }
            AddCenter.shared.receive(paths)
        }
    }

    /// Closing the window keeps the menu bar item and running apps.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    /// Quitting the launcher keeps apps running unless the setting is off;
    /// a running build is paused.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        let store = AppContainer.store
        let keep = LauncherDefaults.keepRunning
        let plan = QuitPlanner.plan(keepRunning: keep, apps: store.apps, building: store.isBuilding)
        guard !plan.isEmpty, !quitting else { return .terminateNow }
        quitting = true
        Task {
            await store.performQuit(keepRunning: keep)
            store.stopEvents()
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }

    /// The app-level alert for a paddock of the wrong version. A missing
    /// paddock shows in the onboarding window instead.
    private func showVersionAlertIfNeeded() {
        guard case .found(_, let verdict) = AppContainer.store.binary,
              let error = VersionGate.current.error(for: verdict), error != .notInstalled else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = error.title?.localized ?? ""
        alert.informativeText = error.body?.localized ?? ""
        // A paddock that did not answer has no command to copy.
        let command = error.command
        if command != nil { alert.addButton(withTitle: LauncherError.Primary.copyCommand.title!.localized) }
        alert.addButton(withTitle: String(localized: "Check Again"))
        alert.addButton(withTitle: String(localized: "Quit"))
        // The index of the button in the full list: Copy, Check Again, Quit.
        let order: [NSApplication.ModalResponse] = [.alertFirstButtonReturn, .alertSecondButtonReturn,
                                                     .alertThirdButtonReturn]
        let shown = order.firstIndex(of: alert.runModal()) ?? order.count
        switch shown + (command == nil ? 1 : 0) {
        case 0:
            if let command { Pasteboard.copy(command) }
            showVersionAlertIfNeeded()
        case 1:
            Task {
                await AppContainer.checkAgain()
                showVersionAlertIfNeeded()
            }
        default:
            NSApp.terminate(nil)
        }
    }

    // MARK: Notifications

    private func notifyWaitingQuestions() {
        guard !AppContainer.isUITest else { return }
        let store = AppContainer.store
        let due = QuestionNotifier.due(store.questions, now: Date(), threshold: LauncherDefaults.notifyAfter,
                                       notified: notified)
        guard !due.isEmpty else { return }
        let center = UNUserNotificationCenter.current()
        center.requestAuthorization(options: [.alert, .sound]) { _, _ in }
        for question in due {
            notified.insert(question.id)
            let content = UNMutableNotificationContent()
            content.title = QuestionNotifier.title(question).localized
            content.body = QuestionsCopy.title(question).localized
            content.userInfo = ["question": question.id]
            center.add(UNNotificationRequest(identifier: question.id, content: content, trigger: nil))
        }
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        Task { @MainActor in
            AppContainer.showMain(.questions)
            completionHandler()
        }
    }
}
