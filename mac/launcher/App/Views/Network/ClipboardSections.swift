import PaddockKit
import SwiftUI

/// The clipboard of an app: the two directions saved in its profile, and
/// while it runs the two directions of this session only.
@MainActor
struct ClipboardSections: View {
    let net: NetworkModel

    var body: some View {
        Section {
            if let state = net.clipboard {
                directions(state.profile, id: "clipboard") { value in
                    Task { await net.setClipboard(profile: value) }
                }
            } else {
                NetStateRow(state: net.clipboardState, subject: .clipboard) {
                    Task { await net.loadClipboard(force: true) }
                }
            }
        } header: {
            Text("Clipboard for this app")
        } footer: {
            if net.clipboard?.running == true {
                Text("Applies now and at every start.")
            } else {
                Text("Applies at the next start.")
            }
        }
        .task { await net.loadClipboard(force: true) }

        if let state = net.clipboard, state.running {
            Section {
                directions(state.session ?? state.profile, id: "session-clipboard") { value in
                    Task { await net.setClipboard(session: value) }
                }
                if (state.session ?? state.profile) == state.profile {
                    Text("Same as the app")
                        .font(.caption)
                        .labelInk()
                        .accessibilityIdentifier("session-same")
                }
            } header: {
                Text("This session only")
            } footer: {
                Text("A change here ends when the app stops. The next start uses the setting of the app.")
            }
        }
    }

    @ViewBuilder
    private func directions(_ value: ClipboardJSON, id: String, set: @escaping (ClipboardJSON) -> Void) -> some View {
        Toggle(isOn: Binding(get: { value.toApp },
                             set: { on in set(ClipboardJSON(toApp: on, fromApp: value.fromApp)) })) {
            Text("Paste from this Mac into the app")
            Text("The app can read what you copy on this Mac when you paste in it.")
        }
        .accessibilityIdentifier(id + "-to-app")
        Toggle(isOn: Binding(get: { value.fromApp },
                             set: { on in set(ClipboardJSON(toApp: value.toApp, fromApp: on)) })) {
            Text("Copy from the app to this Mac")
            Text("What you copy in the app reaches the clipboard of this Mac.")
        }
        .accessibilityIdentifier(id + "-from-app")
    }
}
