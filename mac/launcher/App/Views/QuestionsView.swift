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
            if let issue = store.questionsIssue {
                Text(verbatim: issue).font(.caption).padding()
                    .accessibilityIdentifier("questions-incomplete")
            }
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
                .disabled(!store.pausedConfirmed)
            }
            if store.questions.isEmpty {
                PlainEmpty {
                    Label { Text(store.questionsComplete ? "No open questions" : "Questions are not confirmed") } icon: { DecorativeSymbol(name: "checkmark.shield") }
                } description: {
                    Text("When an app wants to reach a new server, the question appears here and next to the app's window.")
                } actions: {
                    EmptyView()
                }
                .accessibilityElement(children: .contain)
                .accessibilityLabel(Text(store.questionsComplete ? "No open questions" : "Questions are not confirmed"))
            } else {
                List {
                    ForEach(store.questions) { question in
                        QuestionRow(question: question)
                    }
                }
                .accessibilityLabel(Text("Questions"))
                .accessibilityIdentifier("questions-list")
            }
        }
        .navigationTitle(Text("Questions"))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Questions"))
    }
}

/// One open question, with the answers of paddock's panel in the panel's
/// order: `Deny`, `Allow once`, `Allow <site>`, and under `More…` the
/// session, permanent and ten-minute answers. No answer is on the Return
/// key: a reflex Return must not write a permanent rule.
@MainActor
struct QuestionRow: View {
    let question: QuestionJSON
    @Environment(LauncherStore.self) private var store
    @State private var failure: String?
    @State private var working = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            AppIcon(path: store.app(named: question.app)?.json.bundlePath, size: 32)
            VStack(alignment: .leading, spacing: 2) {
                Text(QuestionChoices.title(question))
                    .accessibilityIdentifier("question-title-" + question.id)
                Text(QuestionChoices.detail(question)).font(.caption).labelInk()
                if let process = question.process {
                    Text(verbatim: process).font(.caption).labelInk()
                }
                if let opened = question.opened {
                    TimelineView(.periodic(from: .now, by: 30)) { context in
                        Text(Format.age(since: opened, now: context.date))
                            .font(.caption)
                            .labelInk()
                    }
                }
                if let failure {
                    ProblemLine {
                        Text(verbatim: failure)
                            .font(.caption)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("answer-failed-" + question.id)
                    }
                }
            }
            Spacer()
            Menu {
                ForEach(QuestionChoices.menu(question)) { choice in
                    if choice.answer == .allowEverything { Divider() }
                    button(choice)
                }
            } label: {
                Text("More…")
            }
            .menuAuditAction()
            .accessibilityIdentifier("answer-more-" + question.id)
            ForEach(QuestionChoices.row(question)) { choice in
                button(choice)
                    .buttonStyle(.bordered)
                    .tint(choice.answer == .allowSite ? Color.accentColor : nil)
            }
        }
        .disabled(working || !store.confirmedQuestionIDs.contains(question.id))
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text(QuestionChoices.title(question)))
        .accessibilityIdentifier("question-" + question.id)
    }

    private func button(_ choice: QuestionChoice) -> some View {
        Button { answer(choice.answer) } label: { Text(choice.title) }
            .accessibilityLabel(Text(choice.label))
            .accessibilityIdentifier("answer-" + choice.answer.rawValue + "-" + question.id)
    }

    private func answer(_ answer: PanelAnswer) {
        working = true
        Task {
            failure = await store.answer(question, panel: answer)
            working = false
        }
    }
}
