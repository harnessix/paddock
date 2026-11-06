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
                        .accessibilityIdentifier("build-button")
                    CommandEcho(args: PaddockArgs.setup(dir: nil, ipsw: nil, resume: false))
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
        .accessibilityIdentifier("base-image")
    }

    @ViewBuilder
    private func readySection(_ info: GoldenInfo) -> some View {
        Section {
            Text(BuildCopy.readyLine(info)).accessibilityIdentifier("golden-ready-line")
            if let dir = info.dir {
                LabeledContent("Location") {
                    HStack {
                        Text(verbatim: Format.tildePath(dir, home: NSHomeDirectory()))
                        Button("Reveal in Finder") { SystemLinks.reveal(dir) }
                    }
                }
            }
            Text(OverviewCopy.appsUsing(info.appsUsing))
        }
        Section {
            HStack {
                Button("Check for newer macOS") { Task { await store.checkLatest() } }
                if let line = store.latestCheck {
                    Text(line)
                }
                if store.latestAvailable != nil {
                    Button("Rebuild…") { router.sheet = .rebuild }
                }
            }
            HStack {
                Button("Remove Base Image…", role: .destructive) { router.sheet = .removeGolden }
                    .disabled(!store.activeApps.isEmpty)
                if !store.activeApps.isEmpty {
                    Text("Stop the running apps first.")
                }
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
                let status = progress.status(of: step)
                HStack(alignment: .top) {
                    StatusSymbol(name: status.symbol, tint: status.tint, label: status.accessibilityLabel)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(step.title)
                        if let detail = progress.detail(of: step) {
                            Text(detail).font(.callout)
                                .accessibilityIdentifier("step-detail-" + step.rawValue)
                        }
                        if status == .current || status == .paused {
                            if step == .download, let fraction = progress.fraction {
                                ProgressView(value: fraction)
                                    .accessibilityLabel(Text(step.title))
                            } else if step != .download {
                                ProgressView()
                                    .progressViewStyle(.linear)
                                    .accessibilityLabel(Text(step.title))
                            }
                        }
                        if step == .guided, status == .current {
                            Button("Show Window") { Task { await store.showSetupWindow() } }
                        }
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("step-" + step.rawValue)
            }
            HStack {
                if progress.step == .download {
                    if progress.paused {
                        Button("Resume") { store.resumeBuild() }
                            .accessibilityIdentifier("resume-button")
                    } else {
                        Button("Pause") { Task { await store.pauseBuild() } }
                            .accessibilityIdentifier("pause-button")
                    }
                }
                Button("Cancel") {
                    if let cancel { cancel() } else { router.sheet = .cancelBuild }
                }
                .accessibilityIdentifier("cancel-build")
            }
            Text(BuildCopy.closeNote)
                .font(.callout)
                .accessibilityIdentifier("close-note")
        } header: {
            Text("Building the base image")
        }
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
                            if let path = store.goldenLogPath { SystemLinks.reveal(path) }
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
