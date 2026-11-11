import AppKit
import PaddockKit
import SwiftUI

/// Stop with open questions: the one case that asks first.
@MainActor
struct ConfirmStopSheet: View {
    let name: String
    @Environment(LauncherStore.self) private var store
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ConfirmLayout(title: Text(UIText("Stop %@?", name)),
                      message: Text("Stopping saves the app's state so it reopens in about 10 s. Its open questions are denied."),
                      action: Text("Stop"), destructive: false) {
            await store.stopApp(name)
        }
    }
}

@MainActor
struct ClearStateSheet: View {
    let name: String
    @Environment(LauncherStore.self) private var store

    var body: some View {
        ConfirmLayout(title: Text(AppDetailModel.clearText(app: name)), message: nil, action: Text("Clear"), destructive: true) {
            await store.detail(for: name).clearSavedState()
        }
    }
}

@MainActor
struct RemoveGoldenSheet: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        if let info = store.golden.info {
            ConfirmLayout(title: Text(BuildCopy.removeQuestion(info)), message: nil, action: Text("Remove"), destructive: true) {
                _ = await store.removeBaseImage()
            }
        }
    }
}

@MainActor
struct RebuildSheet: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        ConfirmLayout(title: Text(BuildCopy.rebuildQuestion), message: nil, action: Text("Rebuild"), destructive: false) {
            store.build(rebuild: true)
            store.selection = .baseImage
        }
    }
}

@MainActor
struct CancelBuildSheet: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        ConfirmLayout(title: Text(BuildCopy.cancelQuestion), message: nil, action: Text("Stop"), destructive: true,
                      cancelTitle: Text("Keep Building")) {
            store.cancelBuild()
        }
    }
}

/// A sheet with a question, Cancel and one action. The action is never the
/// default button when it is destructive; Escape cancels.
@MainActor
struct ConfirmLayout: View {
    let title: Text
    let message: Text?
    let action: Text
    let destructive: Bool
    var cancelTitle = Text("Cancel")
    let perform: () async -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var working = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            title.font(.headline).fixedSize(horizontal: false, vertical: true)
            if let message { message.fixedSize(horizontal: false, vertical: true) }
            HStack {
                Spacer()
                Button(role: .cancel) { dismiss() } label: { cancelTitle }
                    .keyboardShortcut(.cancelAction)
                if destructive {
                    Button(role: .destructive) { run() } label: { action }
                        .disabled(working)
                        .accessibilityIdentifier("confirm-action")
                } else {
                    Button { run() } label: { action }
                        .keyboardShortcut(.defaultAction)
                        .disabled(working)
                        .accessibilityIdentifier("confirm-action")
                }
            }
        }
        .padding(20)
        .frame(width: 440)
    }

    private func run() {
        working = true
        Task {
            await perform()
            dismiss()
        }
    }
}
