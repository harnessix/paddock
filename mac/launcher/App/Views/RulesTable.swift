import PaddockKit
import SwiftUI

/// The host rules of an app: Host, Answer, Scope, Added, Last used, Count.
/// Seeded rules from the app profile carry a caption and cannot be
/// removed, only overridden by a Deny rule.
@MainActor
struct RulesTable: View {
    let model: AppDetailModel
    @Binding var selection: Set<RuleJSON.ID>
    @Binding var confirmDelete: Bool

    var body: some View {
        Table(model.rules, selection: $selection) {
            TableColumn("Host") { rule in
                // The caption tells seeded rules apart, in the label color.
                VStack(alignment: .leading) {
                    Text(verbatim: rule.host)
                    if rule.seeded == true {
                        Text("From the app profile").font(.caption)
                    }
                }
            }
            TableColumn("Answer") { rule in Text(RuleText.answer(rule)) }
            TableColumn("Scope") { rule in Text(RuleText.scope(rule)) }
            TableColumn("Added") { rule in
                if let added = rule.added { Text(added, format: .dateTime.day().month()) }
            }
            TableColumn("Last used") { rule in
                Text(OverviewCopy.used(rule.lastUsed, now: Date()))
            }
            TableColumn("Count") { rule in Text(rule.count ?? 0, format: .number).monospacedDigit() }
        }
        .contextMenu(forSelectionType: RuleJSON.ID.self) { ids in
            let rules = model.rules.filter { ids.contains($0.id) }
            if rules.count == 1, let rule = rules.first {
                Button { Task { await model.flipRule(rule) } } label: { Text(RuleText.flipTitle(rule)) }
            }
            if rules.contains(where: { $0.seeded != true }) {
                Button("Remove", role: .destructive) {
                    selection = ids
                    confirmDelete = true
                }
            }
        }
        .onDeleteCommand {
            if model.rules.contains(where: { selection.contains($0.id) && $0.seeded != true }) {
                confirmDelete = true
            }
        }
        .accessibilityIdentifier("rules-table")
    }
}

/// `Add Rule…`: one Host field, Answer and Scope. Validation is inline.
@MainActor
struct AddRuleSheet: View {
    let model: AppDetailModel
    @Environment(\.dismiss) private var dismiss
    @State private var host = ""
    @State private var answer: RuleAnswer = .allow
    @State private var scope: RuleScope = .always
    @State private var message: UIText?
    @State private var working = false

    var body: some View {
        Form {
            Section {
                TextField("Host", text: $host, prompt: Text("api.example.com"))
                    .accessibilityIdentifier("rule-host")
                    .onSubmit(add)
                if let message {
                    ProblemLine {
                        Text(message)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("rule-message")
                    }
                }
                Picker("Answer", selection: $answer) {
                    ForEach(RuleAnswer.allCases) { Text($0.title).tag($0) }
                }
                Picker("Scope", selection: $scope) {
                    ForEach(RuleScope.allCases) { Text($0.title).tag($0) }
                }
                CommandEcho(args: PaddockArgs.ruleAdd(model.name, host: host.isEmpty ? "HOST" : host,
                                                      answer: answer, scope: scope))
            } header: {
                Text("Add Rule")
            } footer: {
                Text("For example api.openai.com, or *.openai.com for every server of that domain.")
            }
        }
        .formStyle(.grouped)
        .frame(width: 460)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Add", action: add)
                    .disabled(host.trimmingCharacters(in: .whitespaces).isEmpty || working)
                    .accessibilityIdentifier("rule-add")
            }
        }
    }

    private func add() {
        working = true
        Task {
            message = await model.addRule(host: host, answer: answer, scope: scope)
            working = false
            if message == nil { dismiss() }
        }
    }
}
