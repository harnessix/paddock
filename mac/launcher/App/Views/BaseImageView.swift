import PaddockKit
import SwiftUI

/// The base image (the golden in paddock's words): none, building, failed,
/// ready or needs-update.
@MainActor
struct BaseImageView: View {
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router

    var body: some View {
        Form {
            switch store.golden {
            case .none:
                Section {
                    Text("No base image").font(.headline)
                    Text(BuildCopy.body)
                    Button("Build…") { store.build() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(!store.binary.isUsable)
                        .accessibilityIdentifier("build-button")
                    if !store.binary.isUsable {
                        ProblemLine { Text(LibraryCopy.offline) }
                    }
                    CommandEcho(args: LibraryArgs.setup(ipsw: nil, resume: false))
                }
            case .building(let progress):
                BuildProgressSection(progress: progress)
            case .failed(let failure):
                BuildFailureSection(failure: failure)
            case .ready(let info):
                readySection(info)
            case .needsUpdate(let info, let host):
                Section {
                    Label { Text(BuildCopy.needsUpdate(host: host, image: info.version)) } icon: {
                        Image(systemName: "info.circle").accessibilityHidden(true)
                    }
                }
                readySection(info)
            }
        }
        .formStyle(.grouped)
        .navigationTitle(Text("Base image"))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Base image"))
        .accessibilityIdentifier("base-image")
    }

    @ViewBuilder
    private func readySection(_ info: GoldenInfo) -> some View {
        Section {
            Text(BuildCopy.readyLine(info)).accessibilityIdentifier("golden-ready-line")
            if let dir = info.dir {
                LabeledContent("Location") {
                    HStack {
                        Text(verbatim: Format.tildePath(dir, home: NSHomeDirectory())).labelInk()
                        Button("Reveal in Finder") { SystemLinks.reveal(dir) }
                    }
                }
            }
            Text(OverviewCopy.appsUsing(info.appsUsing))
        }
        Section {
            if case .needsUpdate(_, let host) = store.golden {
                HStack {
                    Text(UIText("macOS %@ can be built for the apps.", host))
                    Spacer()
                    Button("Update…") { router.sheet = .rebuild }
                        .disabled(store.isBuilding || store.buildRunning)
                        .accessibilityIdentifier("update-golden")
                }
            }
            if let note = store.buildNote {
                ProblemLine { Text(note).accessibilityIdentifier("build-note") }
            }
            HStack {
                let busy = store.apps.contains { $0.state.holdsSession }
                Button("Remove Base Image…", role: .destructive) { router.sheet = .removeGolden }
                    .disabled(busy)
                if busy {
                    Text("Stop the running apps first.")
                }
                Spacer()
                SettingsLink { Text("Storage…") }
            }
        }
    }
}

/// The steps of a build with their symbols, the download bar, Pause /
/// Resume and Cancel. The onboarding window shows the same section.
@MainActor
struct BuildProgressSection: View {
    let progress: BuildProgress
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router
    var cancel: (() -> Void)?

    var body: some View {
        Section {
            ForEach(BuildStep.allCases, id: \.self) { step in
                stepRow(step)
            }
            // Pause and Resume stay in the tree for the whole build. Adding
            // or removing them while the accessibility audit runs makes the
            // parent lose a child.
            HStack {
                Button("Pause") { Task { await store.pauseBuild() } }
                    .disabled(!canPause)
                    .accessibilityIdentifier("pause-button")
                Button("Resume") { store.resumeBuild() }
                    .disabled(!canResume)
                    .accessibilityIdentifier("resume-button")
                Button("Cancel") {
                    if let cancel { cancel() } else { router.sheet = .cancelBuild }
                }
                .accessibilityIdentifier("cancel-build")
            }
            if let note = store.buildNote {
                ProblemLine { Text(note).labelInk().accessibilityIdentifier("build-note") }
            }
            Text(BuildCopy.closeNote)
                .font(.callout)
                .labelInk()
                .accessibilityIdentifier("close-note")
        } header: {
            Text("Building the base image")
        }
    }

    private var canPause: Bool { progress.step == .download && !progress.paused }
    private var canResume: Bool { progress.step == .download && progress.paused }

    private func stepRow(_ step: BuildStep) -> some View {
        let status = progress.status(of: step)
        let current = status == .current || status == .paused
        let detail = progress.detail(of: step)?.localized ?? ""
        // Future steps stay in the tree with an empty line. Putting their
        // text in early would make the UI tests match before that step.
        let guided = step == .guided && status == .current
            ? String(localized: "Follow the steps in paddock's setup window.")
            : ""
        let bar: Double? = step == .download ? (progress.fraction ?? 0) : nil
        return HStack(alignment: .top) {
            StatusSymbol(name: status.symbol, tint: status.tint, label: status.accessibilityLabel)
            VStack(alignment: .leading, spacing: 4) {
                Text(step.title).labelInk()
                Text(verbatim: detail)
                    .font(.callout)
                    .labelInk()
                    .accessibilityIdentifier("step-detail-" + step.rawValue)
                    .accessibilityLabel(Text(verbatim: detail.isEmpty ? status.accessibilityLabel.localized : detail))
                ProgressView(value: bar, total: 1.0)
                    .progressViewStyle(.linear)
                    .frame(height: current ? nil : 0)
                    .clipped()
                    .accessibilityLabel(Text(step.title))
                Text(verbatim: guided)
                    .font(.callout)
                    .labelInk()
                    .accessibilityLabel(Text(verbatim: guided.isEmpty ? status.accessibilityLabel.localized : guided))
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("step-" + step.rawValue)
    }
}

/// `Base image not built`, paddock's reason, and the next step.
@MainActor
struct BuildFailureSection: View {
    let failure: BuildFailure
    @Environment(LauncherStore.self) private var store

    var body: some View {
        Section {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.title2)
                    .foregroundStyle(.orange)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 6) {
                    Text("Base image not built").font(.headline)
                    Text(verbatim: failure.reason).accessibilityIdentifier("failure-reason")
                    if let hint = failure.hint { Text(hint) }
                    HStack {
                        Button { primary() } label: { Text(failure.primary.buttonTitle) }
                            .accessibilityIdentifier("failure-primary")
                        if failure.primary != .tryAgain {
                            Button("Try Again") { store.resumeBuild() }
                        }
                        Button("Show Log") {
                            SystemLinks.reveal(store.goldenLogPath ?? store.golden.info?.dir ?? store.logDir
                                ?? NSHomeDirectory() + "/Library/Application Support/paddock/macvm")
                        }
                    }
                }
            }
        }
    }

    private func primary() {
        switch failure.primary {
        case .manageStorage: SystemLinks.manageStorage()
        case .tryAgain: store.resumeBuild()
        case .chooseRestoreImage:
            if let ipsw = Pickers.restoreImage() { store.build(ipsw: ipsw) }
        }
    }
}
