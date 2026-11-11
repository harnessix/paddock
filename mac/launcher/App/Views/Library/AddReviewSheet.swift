import AppKit
import PaddockKit
import SwiftUI

/// Add App: the app, paddock's dry run as a checklist, then `Add`. The
/// same sheet for `+`, Command-N, a drop on the window and a drop on the
/// Dock icon. Queued drops come one after another.
@MainActor
struct AddReviewSheet: View {
    let initialPath: String?
    @Environment(LauncherStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var path: String?
    @State private var review: AddReview?
    @State private var name = ""
    @State private var memory = 6
    @State private var adding = false
    @State private var refusal: String?
    private var center: AddCenter { AddCenter.shared }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Form {
                if let position = center.queue.position, initialPath != nil {
                    Text(position).font(.callout).accessibilityIdentifier("add-position")
                }
                if let path {
                    header(path)
                    if let review {
                        checklist(review)
                        if review.canAdd { choices(review) }
                    } else {
                        Section {
                            ProgressView { Text("Checking the app…") }
                                .accessibilityIdentifier("add-checking")
                        }
                    }
                } else {
                    Section { ProgressView { Text("Choose an app…") } }
                }
                if let refusal {
                    Section {
                        ProblemLine { Text(verbatim: refusal).accessibilityIdentifier("add-refusal") }
                    }
                }
            }
            .formStyle(.grouped)
            Divider()
            buttons.padding(16)
        }
        .frame(width: 500)
        .frame(minHeight: 380)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Add App…"))
        .task { await start() }
        .onDisappear { center.finished(initialPath) }
    }

    // MARK: Parts

    private func header(_ path: String) -> some View {
        Section {
            HStack(spacing: 12) {
                AppIcon(path: path, size: 48)
                VStack(alignment: .leading, spacing: 2) {
                    Text(verbatim: review?.displayName ?? AddReview.slugBase(path)).font(.headline)
                    Text(verbatim: Format.tildePath(path, home: NSHomeDirectory()))
                        .font(.callout)
                        .labelInk()
                        .lineLimit(1)
                        .truncationMode(.middle)
                    if let bytes = review?.bytes {
                        Text(verbatim: Format.gb(bytes)).font(.callout).labelInk()
                    }
                }
            }
            .accessibilityElement(children: .combine)
        }
    }

    @ViewBuilder
    private func checklist(_ review: AddReview) -> some View {
        Section {
            if let failure = review.failure {
                ProblemLine { Text(verbatim: failure).accessibilityIdentifier("add-message") }
            }
            if let existing = review.existing {
                HStack {
                    Text(LibraryCopy.existing(existing)).accessibilityIdentifier("add-message")
                    Spacer()
                    Button("Show App") {
                        store.selection = .app(existing)
                        dismiss()
                    }
                }
            }
            ForEach(review.checks) { check in
                CheckRow(check: check, isFirstFailure: check.id == review.firstFailure?.id, dismiss: { dismiss() })
            }
        } header: {
            Text("Before it is added")
        }
    }

    private func choices(_ review: AddReview) -> some View {
        Section {
            TextField("Name", text: $name)
                .accessibilityIdentifier("add-name")
            Picker("Memory", selection: $memory) {
                ForEach(review.memoryOptions, id: \.self) { gb in
                    Text(verbatim: Format.memoryGB(Int64(gb) * 1_073_741_824)).tag(gb)
                }
            }
            .menuAuditAction()
            CommandEcho(args: LibraryArgs.add(review.path, name: name, memoryGB: memory))
        } footer: {
            Text("The app gets a launcher in your Applications folder, so it also starts from the Dock and Launchpad.")
        }
    }

    private var buttons: some View {
        HStack {
            if center.queue.total > 1, initialPath != nil {
                Button("Cancel All") {
                    center.cancelAll()
                    dismiss()
                }
            }
            Spacer()
            Button("Cancel", role: .cancel) { dismiss() }
                .keyboardShortcut(.cancelAction)
            Button("Add") { add() }
                .keyboardShortcut(.defaultAction)
                .disabled(review?.canAdd != true || trimmedName.isEmpty || adding)
                .accessibilityIdentifier("add-confirm")
        }
    }

    private var trimmedName: String { name.trimmingCharacters(in: .whitespaces) }

    // MARK: Actions

    private func start() async {
        guard path == nil else { return }
        guard let picked = initialPath ?? Pickers.app() else { return dismiss() }
        path = picked
        let result = await store.review(path: picked)
        review = result
        name = result.name
        memory = result.defaultMemory
    }

    private func add() {
        guard let review else { return }
        adding = true
        refusal = nil
        Task {
            let outcome = await store.add(path: review.path, name: trimmedName, memoryGB: memory)
            adding = false
            switch outcome {
            case .added: dismiss()
            case .refused(let line): refusal = line
            }
        }
    }
}

/// One row of the review checklist: symbol, title, text, and the action
/// of a failed row.
@MainActor
struct CheckRow: View {
    let check: AddCheck
    let isFirstFailure: Bool
    let dismiss: () -> Void
    @Environment(LauncherStore.self) private var store

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            StatusSymbol(name: check.symbol, tint: check.tint, label: check.accessibilityLabel)
            VStack(alignment: .leading, spacing: 4) {
                Text(check.title).font(.headline)
                Text(check.detail)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier(isFirstFailure ? "add-message" : "check-detail-" + check.id)
                if let action = check.action { actionButton(action) }
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text(check.title))
        .accessibilityIdentifier("check-" + check.id)
    }

    @ViewBuilder
    private func actionButton(_ action: AddCheck.Action) -> some View {
        switch action {
        case .openWebsite(let site):
            if let url = URL(string: site) {
                Button("Open website") { SystemLinks.open(url) }
            }
        case .build:
            Button("Build…") {
                store.selection = .baseImage
                AddCenter.shared.cancelAll()
                dismiss()
            }
            .accessibilityIdentifier("add-build")
        case .manageStorage:
            Button("Manage Storage…") { SystemLinks.manageStorage() }
        }
    }
}
