import PaddockKit
import SwiftUI

/// The firewall rules of an app: Host, Port, Answer, Scope, Made by, Time
/// left, Last used. A site pair (`example.com` and `*.example.com`) is one
/// row. Rules of the app profile carry a caption and cannot be edited or
/// removed, only overridden by a Deny rule.
@MainActor
struct RulesTable: View {
    let rows: [RuleRow]
    let now: Date
    @Binding var selection: Set<RuleRow.ID>
    let edit: (RuleRow) -> Void
    let denyInstead: (RuleRow) -> Void
    let remove: ([RuleRow]) -> Void

    var body: some View {
        // A SwiftUI Table wraps every cell in a group with no description.
        // A row of texts keeps Host, Port and the rest as static texts, and
        // the row itself has a label.
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text("Host").frame(minWidth: 160, alignment: .leading)
                Text("Port").frame(minWidth: 60, alignment: .leading)
                Text("Answer").frame(minWidth: 70, alignment: .leading)
                Text("Scope").frame(minWidth: 90, alignment: .leading)
                Text("Made by").frame(minWidth: 90, alignment: .leading)
                Text("Time left").frame(minWidth: 70, alignment: .leading)
                Text("Last used").frame(minWidth: 80, alignment: .leading)
            }
            .font(.caption)
            .labelInk()
            .padding(.horizontal, 6)
            .padding(.bottom, 4)
            .accessibilityHidden(true)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(rows) { row in line(row) }
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Rules"))
        .accessibilityIdentifier("rules-table")
        .onDeleteCommand {
            let chosen = rows.filter { selection.contains($0.id) }
            if chosen.contains(where: { !$0.isProfile }) { remove(chosen) }
        }
    }

    private func line(_ row: RuleRow) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            VStack(alignment: .leading, spacing: 1) {
                pick(Text(NetRuleCopy.host(row)), row)
                if row.isProfile {
                    // The caption tells profile rules apart, in the label color.
                    pick(Text("From the app profile").font(.caption), row)
                }
            }
            .frame(minWidth: 160, alignment: .leading)
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text(NetRuleCopy.host(row)))
            pick(Text(NetRuleCopy.port(row.port)).monospacedDigit(), row)
                .frame(minWidth: 60, alignment: .leading)
            pick(VerdictLabel(verdict: row.verdict), row)
                .frame(minWidth: 70, alignment: .leading)
            pick(Text(NetRuleCopy.scope(row.scope)), row)
                .frame(minWidth: 90, alignment: .leading)
            pick(Text(row.maker.title), row)
                .frame(minWidth: 90, alignment: .leading)
            pick(Text(verbatim: NetRuleCopy.timeLeft(row, now: now)?.localized ?? ""), row)
                .frame(minWidth: 70, alignment: .leading)
                .accessibilityIdentifier("rule-time-left")
            pick(Text(OverviewCopy.used(row.lastUsed, now: now)), row)
                .frame(minWidth: 80, alignment: .leading)
        }
        .padding(.vertical, 4)
        .padding(.horizontal, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(selection.contains(row.id) ? Color.accentColor.opacity(0.18) : Color.clear)
        .contentShape(Rectangle())
        .onTapGesture { selection = [row.id] }
        .contextMenu {
            if row.isProfile {
                Button("Deny Instead…") { denyInstead(row) }
            } else {
                Button("Edit…") { edit(row) }
                Button("Remove…", role: .destructive) { remove([row]) }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text(NetRuleCopy.host(row)))
    }

    /// A click on the words selects the row. The static text stays in the tree.
    private func pick<V: View>(_ view: V, _ row: RuleRow) -> some View {
        view.labelInk().onTapGesture { selection = [row.id] }
    }
}

/// Allow or Deny: a symbol and the word. Color is never the only signal.
@MainActor
struct VerdictLabel: View {
    let verdict: String

    var body: some View {
        Label {
            Text(NetRuleCopy.verdict(verdict))
        } icon: {
            Image(systemName: verdict == "deny" ? "nosign" : "checkmark.circle")
                .foregroundStyle(verdict == "deny" ? Color.red : Color.green)
                .accessibilityHidden(true)
        }
    }
}

/// What the rule editor edits.
enum RuleEditorTarget: Identifiable {
    case add(RuleDraft)
    case edit(RuleRow)

    var id: String {
        switch self {
        case .add(let draft): return "add|" + draft.host + "|" + draft.verdict.rawValue
        case .edit(let row): return "edit|" + row.id
        }
    }

    var draft: RuleDraft {
        switch self {
        case .add(let draft): return draft
        case .edit(let row): return RuleDraft(row: row)
        }
    }

    var row: RuleRow? {
        if case .edit(let row) = self { return row }
        return nil
    }
}

/// `Add Rule…` and `Edit…`: Host, the whole site, Port, Answer, Scope.
/// Validation is inline; paddock's refusal shows verbatim.
@MainActor
struct RuleEditorSheet: View {
    let net: NetworkModel
    let target: RuleEditorTarget
    @Environment(\.dismiss) private var dismiss
    @State private var draft: RuleDraft
    @State private var message: NetMessage?
    @State private var working = false

    init(net: NetworkModel, target: RuleEditorTarget) {
        self.net = net
        self.target = target
        _draft = State(initialValue: target.draft)
    }

    var body: some View {
        Form {
            Section {
                TextField("Host", text: $draft.host, prompt: Text("api.example.com"))
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("rule-host")
                    .onSubmit(save)
                if let message {
                    ProblemLine {
                        VStack(alignment: .leading, spacing: 6) {
                            Text(message.text)
                                .fixedSize(horizontal: false, vertical: true)
                                .accessibilityIdentifier("rule-message")
                            if let command = message.command {
                                CopyField(command: command)
                            }
                        }
                    }
                }
                if let site = draft.siteSuggestion {
                    Toggle(isOn: $draft.site) {
                        Text(UIText("Also every subdomain of %@", site))
                        Text("paddock adds one rule for the site and one for every name under it.")
                    }
                    .accessibilityIdentifier("rule-site")
                }
                Picker("Port", selection: $draft.port) {
                    ForEach(PortChoice.allCases) { choice in Text(choice.title).tag(choice) }
                }
                .menuAuditAction()
                .accessibilityIdentifier("rule-port")
                if draft.port == .other {
                    TextField("Port number", text: $draft.otherPort, prompt: Text("8443"))
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("rule-port-number")
                }
                Picker("Answer", selection: $draft.verdict) {
                    ForEach(RuleAnswer.allCases) { answer in Text(answer.title).tag(answer) }
                }
                .pickerStyle(.radioGroup)
                .accessibilityIdentifier("rule-answer")
                Picker("Scope", selection: $draft.scope) {
                    Text("Always").tag(RuleScope.always)
                    if net.isRunning || draft.scope == .session {
                        Text("Until the app stops").tag(RuleScope.session)
                    }
                }
                .menuAuditAction()
                .accessibilityIdentifier("rule-scope")
                if case .success(let rule) = draft.check(app: net.name, running: net.isRunning) {
                    CommandEcho(args: target.row.map { NetworkArgs.ruleChange(net.name, id: $0.ids[0], rule) }
                        ?? NetworkArgs.ruleAdd(net.name, rule))
                }
            } header: {
                if target.row == nil { Text("Add Rule") } else { Text("Edit Rule") }
            } footer: {
                Text("For example api.example.com, *.example.com for every name under example.com, or an IP address.")
            }
        }
        .formStyle(.grouped)
        .frame(width: 480)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(target.row == nil ? Text("Add Rule") : Text("Edit Rule"))
        .onChange(of: draft) { _, _ in message = nil }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(action: save) {
                    if target.row == nil { Text("Add") } else { Text("Save") }
                }
                .disabled(draft.host.trimmingCharacters(in: .whitespaces).isEmpty || working)
                .accessibilityIdentifier("rule-add")
            }
        }
    }

    private func save() {
        guard !working else { return }
        working = true
        Task {
            let result = await net.save(draft, editing: target.row)
            working = false
            if let result {
                message = result
            } else {
                dismiss()
            }
        }
    }
}
