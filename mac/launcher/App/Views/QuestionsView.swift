import PaddockKit
import SwiftUI

/// The open firewall questions. paddock's own panel next to the app's
/// window stays the place to answer; this list mirrors it. The launcher
/// never draws a second panel.
@MainActor
struct QuestionsView: View {
    @Environment(LauncherStore.self) private var store

    var body: some View {
        VStack(spacing: 0) {
            if let paused = store.paused {
                GroupBox {
                    HStack(alignment: .top) {
                        Image(systemName: "pause.circle.fill").accessibilityHidden(true)
                        Text(QuestionsCopy.pausedBanner(paused))
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("paused-banner")
                        Spacer()
                        Button("Deny all") { Task { await store.pausedDenyAll() } }
                            .accessibilityIdentifier("deny-all")
                        Button("Ask one by one") { Task { await store.pausedOneByOne() } }
                            .accessibilityIdentifier("ask-one-by-one")
                    }
                    .padding(4)
                }
                .backgroundStyle(.yellow.opacity(0.25))
                .padding()
            }
            if store.questions.isEmpty {
                ContentUnavailableView {
                    Label { Text("No open questions") } icon: { DecorativeSymbol(name: "checkmark.shield") }
                } description: {
                    Text("When an app wants to reach a new server, the question appears here and next to the app's window.")
                }
            } else {
                List {
                    ForEach(store.questions) { question in
                        QuestionRow(question: question)
                    }
                }
                .accessibilityIdentifier("questions-list")
            }
        }
        .navigationTitle(Text("Questions"))
    }
}

@MainActor
struct QuestionRow: View {
    let question: QuestionJSON
    @Environment(LauncherStore.self) private var store

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            AppIcon(path: store.app(named: question.app)?.json.bundlePath, size: 32)
            VStack(alignment: .leading, spacing: 2) {
                Text(QuestionsCopy.title(question))
                if let process = question.process {
                    Text(verbatim: process).font(.caption)
                }
                if let opened = question.opened {
                    TimelineView(.periodic(from: .now, by: 30)) { context in
                        Text(Format.age(since: opened, now: context.date))
                            .font(.caption)
                    }
                }
            }
            Spacer()
            answer(.deny)
            answer(.allowOnce)
            answer(.allowAlways)
            Menu {
                answer(.allowSession)
                answer(.denyAlways)
            } label: {
                Text("More…")
            }
            .fixedSize()
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("question-" + question.id)
    }

    private func answer(_ answer: Answer) -> some View {
        Button { Task { await store.answer(question, answer) } } label: { Text(answer.title) }
            .accessibilityIdentifier("answer-" + answer.rawValue + "-" + question.id)
    }
}
