import PaddockKit
import SwiftUI

/// The open firewall questions. paddock's own panel next to the app's
/// window stays the place to answer; this list mirrors it. The launcher
/// never draws a second panel.
@MainActor
struct QuestionsView: View {
    @Environment(LauncherStore.self) private var store
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

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
                let title: LocalizedStringKey = store.questionsComplete ? "No open questions" : "Questions are not confirmed" // localized
                PlainEmpty {
                    Label { Text(title) } icon: { DecorativeSymbol(name: "checkmark.shield") }
                } description: {
                    Text("When an app wants to reach a new server, the question appears here and next to the app's window.")
                } actions: {
                    EmptyView()
                }
                .accessibilityElement(children: .contain)
                .accessibilityLabel(Text(title))
                .accessibilityIdentifier("questions-empty")
            } else {
                let ordered = QuestionChoices.newestFirst(store.questions)
                ScrollView {
                    LazyVStack(spacing: 12) {
                        ForEach(ordered) { question in
                            QuestionCard(question: question, keys: question.id == ordered.first?.id)
                                .transition(.opacity)
                        }
                    }
                    .padding(16)
                    .animation(reduceMotion ? nil : .default, value: ordered.map(\.id))
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

/// One open question as a card, in the order of Little Snitch: the app
/// icon and `ChatGPT wants to connect to`, the host large, the port and the
/// process small. Then the answers of paddock's panel in the panel's order,
/// all the same weight: `Deny`, `Allow once`, `Allow <site>`, and under
/// `More…` the session, permanent and ten-minute answers.
///
/// The newest card has the keys of the panel (D14): Esc denies, Return
/// allows once. No key writes a permanent rule.
@MainActor
struct QuestionCard: View {
    let question: QuestionJSON
    /// True for the newest card: it takes Esc and Return.
    var keys = false
    @Environment(LauncherStore.self) private var store
    @State private var failure: String?
    @State private var working = false

    var body: some View {
        GroupBox {
            HStack(alignment: .top, spacing: 12) {
                AppIcon(path: store.app(named: question.app)?.json.bundlePath, size: IconSize.question)
                VStack(alignment: .leading, spacing: 4) {
                    Text(QuestionChoices.lead(question))
                        .labelInk()
                        .accessibilityIdentifier("question-title-" + question.id)
                    Text(verbatim: question.host)
                        .font(.title3.monospaced())
                        .labelInk()
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                        .accessibilityIdentifier("question-host-" + question.id)
                    // `port 443 · example.com and all its subdomains`.
                    Text(QuestionChoices.detail(question)).font(.caption).labelInk()
                    if let caption = processLine {
                        TimelineView(.periodic(from: .now, by: 30)) { context in
                            Text(verbatim: caption(context.date))
                                .font(.caption)
                                .labelInk()
                                .lineLimit(1)
                                .truncationMode(.middle)
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
                    HStack(spacing: 8) {
                        ForEach(QuestionChoices.row(question)) { choice in
                            button(choice)
                                .buttonStyle(.bordered)
                                .keyboardShortcut(shortcut(for: choice.answer))
                        }
                        Spacer(minLength: 8)
                        Menu {
                            ForEach(QuestionChoices.menu(question)) { choice in
                                if choice.answer == .allowEverything { Divider() }
                                button(choice)
                            }
                        } label: {
                            Text("More…")
                        }
                        .fixedSize()
                        .menuAuditAction()
                        .accessibilityIdentifier("answer-more-" + question.id)
                    }
                    .padding(.top, 6)
                }
                Spacer(minLength: 0)
            }
            .padding(6)
        }
        .disabled(working || !store.confirmedQuestionIDs.contains(question.id))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text(QuestionChoices.title(question)))
        .accessibilityIdentifier("question-" + question.id)
    }

    /// `Sample Notes.app → helper (Renderer) · 3 min ago`, from the parts
    /// paddock sent.
    private var processLine: ((Date) -> String)? {
        guard question.process != nil || question.opened != nil else { return nil }
        let process = question.process
        let opened = question.opened
        return { now in
            var parts: [String] = []
            if let process { parts.append(process) }
            if let opened { parts.append(Format.age(since: opened, now: now).localized) }
            return parts.joined(separator: " · ")
        }
    }

    private func shortcut(for answer: PanelAnswer) -> KeyboardShortcut? {
        guard keys else { return nil }
        switch answer {
        case .deny: return .cancelAction
        case .allowOnce: return .defaultAction
        default: return nil
        }
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
