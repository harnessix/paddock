import PaddockKit
import SwiftUI

/// What a network list shows: its loading text and its failure texts.
enum NetSubject {
    case rules, connections, clipboard

    var loading: UIText {
        switch self {
        case .rules: return UIText("Loading rules…")
        case .connections: return UIText("Loading connections…")
        case .clipboard: return UIText("Loading clipboard settings…")
        }
    }

    var failedTitle: UIText {
        switch self {
        case .rules: return UIText("The rules did not load")
        case .connections: return UIText("The connections did not load")
        case .clipboard: return UIText("The clipboard settings did not load")
        }
    }

    var updateText: UIText {
        switch self {
        case .rules: return UIText("This paddock cannot list the rules of an app yet. Update it in Terminal, then try again.")
        case .connections: return UIText("This paddock cannot list the connections of an app yet. Update it in Terminal, then try again.")
        case .clipboard: return UIText("This paddock cannot show the clipboard of a session yet. Update it in Terminal, then try again.")
        }
    }
}

/// The screen of a list that is not loaded: loading, paddock offline,
/// paddock too old for the command, or paddock's error with `Try Again`.
@MainActor
struct NetStateView: View {
    let state: NetLoad
    let subject: NetSubject
    let retry: () -> Void

    var body: some View {
        switch state {
        case .idle, .loading:
            ProgressView { Text(subject.loading) }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .accessibilityIdentifier("net-loading")
        case .offline:
            PlainEmpty {
                Label { Text("paddock is not reachable") } icon: { DecorativeSymbol(name: "bolt.horizontal.circle") }
            } description: {
                Text("paddock for Mac cannot run the paddock command. Check that paddock is installed, then check again.")
            } actions: {
                Button("Check Again", action: retry)
                    .accessibilityIdentifier("net-retry")
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text("paddock is not reachable"))
            .accessibilityIdentifier("net-offline")
        case .needsNewerPaddock:
            PlainEmpty {
                Label { Text("Update paddock") } icon: { DecorativeSymbol(name: "arrow.down.circle") }
            } description: {
                Text(subject.updateText)
            } actions: {
                CopyField(command: ShellCommands.brewUpgrade)
                    .frame(maxWidth: 360)
                Button("Try Again", action: retry)
                    .accessibilityIdentifier("net-retry")
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text("Update paddock"))
            .accessibilityIdentifier("net-update")
        case .failed(let line):
            PlainEmpty {
                Label { Text(subject.failedTitle) } icon: {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                        .accessibilityHidden(true)
                }
            } description: {
                Text(verbatim: line)
                    .textSelection(.enabled)
                    .labelInk()
                    .accessibilityIdentifier("net-error-line")
            } actions: {
                Button("Try Again", action: retry)
                    .accessibilityIdentifier("net-retry")
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text(subject.failedTitle))
            .accessibilityIdentifier("net-failed")
        case .loaded:
            EmptyView()
        }
    }
}

/// The same states as one row of a Form section.
@MainActor
struct NetStateRow: View {
    let state: NetLoad
    let subject: NetSubject
    let retry: () -> Void

    var body: some View {
        switch state {
        case .idle, .loading:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(subject.loading)
            }
            .accessibilityElement(children: .combine)
        case .offline:
            row(Text("paddock is not reachable"), action: Text("Check Again"))
        case .needsNewerPaddock:
            VStack(alignment: .leading, spacing: 6) {
                row(Text(subject.updateText), action: Text("Try Again"))
                CopyField(command: ShellCommands.brewUpgrade)
            }
        case .failed(let line):
            row(Text(verbatim: line), action: Text("Try Again"))
        case .loaded:
            EmptyView()
        }
    }

    private func row(_ text: Text, action: Text) -> some View {
        HStack(alignment: .firstTextBaseline) {
            ProblemLine { text.fixedSize(horizontal: false, vertical: true) }
            Spacer()
            Button(action: retry) { action }
                .accessibilityIdentifier("net-retry")
        }
    }
}

/// paddock's line after a failed change, above a list, with the command
/// to copy when the fix is in Terminal.
@MainActor
struct NetProblemBanner: View {
    let message: NetMessage
    let dismiss: () -> Void

    var body: some View {
        GroupBox {
            content.padding(4)
        }
        .accessibilityElement(children: .contain)
    }

    private var content: some View {
        HStack(alignment: .top) {
            ProblemLine {
                VStack(alignment: .leading, spacing: 6) {
                    Text(message.text)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("net-problem")
                    if let command = message.command {
                        CopyField(command: command)
                    }
                }
            }
            Spacer()
            Button(action: dismiss) {
                Image(systemName: "xmark.circle")
            }
            .buttonStyle(.borderless)
            .accessibilityLabel(Text("Close"))
            .help(Text("Close"))
        }
    }
}
