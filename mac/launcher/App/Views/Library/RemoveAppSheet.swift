import PaddockKit
import SwiftUI

/// Remove App: what goes, what stays, and the data choice. The data goes
/// to paddock's trash for 7 days by default. A running or suspended
/// session must stop first.
@MainActor
struct RemoveAppSheet: View {
    let name: String
    @Environment(LauncherStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var deleteData = false
    @State private var working = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let app = store.app(named: name) {
                let instances = store.instances(ofApp: app.instance.app)
                let busy = instances.filter { $0.state.holdsSession }
                Text(LibraryCopy.removeTitle(app, instances: instances.count)).font(.headline)
                Text(LibraryCopy.removeBody(app))
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("remove-body")
                Toggle("Also delete the data now instead of moving it to the trash", isOn: $deleteData)
                    .accessibilityIdentifier("remove-delete-data")
                Text("The login and the files of the app in its sandbox cannot come back.")
                    .font(.caption)
                    .labelInk()
                CommandEcho(args: LibraryArgs.remove(app.instance.app, deleteData: deleteData))
                if !busy.isEmpty {
                    ForEach(busy) { item in
                        HStack {
                            ProblemLine { Text(UIText("Stop %@ first.", item.title)) }
                            Spacer()
                            Button("Stop") { Task { await store.stopApp(item.key) } }
                        }
                    }
                }
                if working {
                    ProgressView { Text("Removing…") }.progressViewStyle(.linear)
                }
                HStack {
                    Spacer()
                    Button("Cancel", role: .cancel) { dismiss() }
                        .keyboardShortcut(.cancelAction)
                    Button("Remove", role: .destructive) { remove() }
                        .disabled(!busy.isEmpty || working)
                        .accessibilityIdentifier("remove-confirm")
                }
            } else {
                // The app went while the sheet was open.
                Text("This app is no longer in paddock.")
                HStack {
                    Spacer()
                    Button("Close") { dismiss() }.keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(20)
        .frame(width: 500)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Remove App…"))
    }

    private func remove() {
        working = true
        Task {
            await store.removeApp(name, deleteData: deleteData)
            working = false
            dismiss()
        }
    }
}
