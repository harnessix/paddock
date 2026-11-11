import PaddockKit
import SwiftUI

/// Settings > Storage: what paddock keeps on this Mac, from `paddock du
/// --json`, and `Clean Up…` with a preview of `paddock gc`.
@MainActor
struct StorageView: View {
    @Environment(LauncherStore.self) private var store
    @State private var model = StorageModel()
    @State private var confirmClear = false

    var body: some View {
        Form {
            switch model.phase {
            case .idle, .loading:
                Section {
                    ProgressView { Text("Measuring…") }
                        .accessibilityIdentifier("storage-loading")
                }
            case .offline:
                Section {
                    ProblemLine { Text("Storage needs paddock. Install or update it, then click Check Again.") }
                    Button("Check Again") {
                        Task {
                            await AppContainer.checkAgain()
                            await model.load(store)
                        }
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("storage-offline")
            case .failed(let line):
                Section {
                    ProblemLine { Text(verbatim: line).accessibilityIdentifier("storage-error") }
                    Button("Try Again") { Task { await model.load(store) } }
                }
            case .empty:
                Section {
                    Text("paddock has no files on this Mac yet.")
                        .accessibilityIdentifier("storage-empty")
                }
            case .loaded(let report):
                usage(report)
                rows(report)
                actions
            }
        }
        .formStyle(.grouped)
        .task { await model.load(store) }
        .sheet(isPresented: Binding(get: { model.cleanup != .closed }, set: { if !$0 { model.closeCleanup() } })) {
            CleanupSheet(model: model)
                .environment(store)
        }
        .confirmationDialog(Text("Clear the saved state of every app?"), isPresented: $confirmClear) {
            Button("Clear", role: .destructive) {
                Task {
                    for app in store.apps where !app.state.holdsSession {
                        await store.detail(for: app.key).clearSavedState()
                    }
                    await model.load(store)
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The next start of each app is cold, about 30 s. Your data in the apps stays.")
        }
    }

    private func usage(_ report: StorageReport) -> some View {
        Section {
            if let free = report.freeBytes {
                ProgressView(value: Double(report.totalBytes), total: Double(max(report.totalBytes + free, 1)))
                    .accessibilityLabel(Text(report.usageLine))
            }
            Text(report.usageLine).labelInk().accessibilityIdentifier("storage-usage")
            LabeledContent("Location") {
                HStack {
                    Text(verbatim: Format.tildePath(report.root, home: NSHomeDirectory()))
                        .labelInk()
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Button("Reveal in Finder") { SystemLinks.reveal(report.root) }
                }
            }
        }
    }

    private func rows(_ report: StorageReport) -> some View {
        Section {
            ForEach(report.rows) { row in
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(row.title)
                        if let detail = row.detail {
                            Text(detail).font(.callout).labelInk()
                        }
                    }
                    Spacer()
                    if let state = row.state {
                        Text(state).font(.callout).labelInk()
                    }
                    Text(verbatim: Format.gb(row.bytes)).labelInk()
                        .monospacedDigit()
                        .frame(minWidth: 70, alignment: .trailing)
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("storage-row-" + row.id)
            }
        } header: {
            Text("What paddock keeps")
        } footer: {
            Text("paddock never removes anything of an app that runs or is suspended.")
        }
    }

    private var actions: some View {
        Section {
            HStack {
                Button("Clean Up…") { Task { await model.preview(store) } }
                    .accessibilityIdentifier("storage-clean-up")
                Button("Clear All Saved States…") { confirmClear = true }
                    .disabled(store.apps.allSatisfy { $0.state.holdsSession })
                Spacer()
                Button("Refresh") { Task { await model.load(store) } }
            }
            CommandEcho(args: LibraryArgs.gc(apply: false, includeLogins: false))
            Text("To move the base image, remove it and build it again in a new location.")
                .font(.callout)
                .labelInk()
        }
    }
}

/// The preview of `paddock gc`: what `Remove` takes now, what stays, then
/// the result.
@MainActor
struct CleanupSheet: View {
    @Bindable var model: StorageModel
    @Environment(LauncherStore.self) private var store
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Clean Up paddock").font(.headline)
            content
        }
        .padding(20)
        .frame(width: 540)
        .frame(minHeight: 320)
    }

    @ViewBuilder private var content: some View {
        switch model.cleanup {
        case .closed, .previewing:
            ProgressView { Text("Looking for files paddock no longer needs…") }
                .accessibilityIdentifier("cleanup-loading")
            footer(canRemove: false)
        case .preview(let preview), .removing(let preview):
            previewList(preview)
            Toggle(isOn: Binding(get: { model.includeLogins }, set: { on in
                model.includeLogins = on
                Task { await model.preview(store) }
            })) {
                Text("Also remove unused apps and their logins")
                Text("An app that was not used for 90 days goes with its login and its files.")
            }
            .accessibilityIdentifier("cleanup-include-logins")
            CommandEcho(args: LibraryArgs.gc(apply: true, includeLogins: preview.includeLogins))
            if case .removing = model.cleanup {
                ProgressView { Text("Removing…") }.progressViewStyle(.linear)
            }
            footer(canRemove: !preview.isEmpty && model.cleanup == .preview(preview), title: preview.removeTitle)
        case .done(let result):
            Label { Text(result.line).accessibilityIdentifier("cleanup-done") } icon: {
                Image(systemName: "checkmark.circle.fill").foregroundStyle(.green).accessibilityHidden(true)
            }
            ForEach(result.failures) { failure in
                ProblemLine { Text(verbatim: failure.what + ": " + failure.line) }
            }
            HStack {
                Spacer()
                Button("Done") { model.closeCleanup() }.keyboardShortcut(.defaultAction)
            }
        case .failed(let line):
            ProblemLine { Text(verbatim: line).accessibilityIdentifier("cleanup-error") }
            HStack {
                Spacer()
                Button("Close") { model.closeCleanup() }.keyboardShortcut(.cancelAction)
                Button("Try Again") { Task { await model.preview(store) } }.keyboardShortcut(.defaultAction)
            }
        }
    }

    @ViewBuilder
    private func previewList(_ preview: CleanupPreview) -> some View {
        if preview.isEmpty {
            Text("Nothing to clean up. paddock keeps only what your apps need.")
                .accessibilityIdentifier("cleanup-nothing")
        }
        List {
            if !preview.removedNow.isEmpty {
                Section {
                    ForEach(preview.removedNow) { item in CleanupRow(item: item) }
                } header: {
                    Text("Removed now")
                }
            }
            if !preview.kept.isEmpty {
                Section {
                    ForEach(preview.kept) { item in CleanupRow(item: item) }
                } header: {
                    Text("Kept unless you turn on the option below")
                }
            }
        }
        .frame(minHeight: 160)
        .accessibilityIdentifier("cleanup-list")
        if preview.hasOtherData {
            Text("The list also has paddock data outside the Mac apps, for example of coding agents. Remove takes it too.")
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func footer(canRemove: Bool, title: UIText? = nil) -> some View {
        HStack {
            Spacer()
            Button("Cancel", role: .cancel) { model.closeCleanup() }
                .keyboardShortcut(.cancelAction)
            Button(role: .destructive) {
                Task { await model.apply(store) }
            } label: {
                if let title { Text(title) } else { Text("Remove") }
            }
            .disabled(!canRemove)
            .accessibilityIdentifier("cleanup-remove")
        }
    }
}

/// One item of the clean up preview: what, why, the warning, the size.
@MainActor
struct CleanupRow: View {
    let item: CleanupPreview.Item

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: item.item.what)
                if let reason = item.item.reason, !reason.isEmpty {
                    Text(verbatim: reason).font(.callout)
                }
                if let warning = item.item.warning, !warning.isEmpty {
                    ProblemLine { Text(verbatim: warning).font(.callout) }
                }
            }
            Spacer()
            Text(verbatim: Format.gb(item.item.bytes)).monospacedDigit()
        }
        .accessibilityElement(children: .combine)
    }
}
