import AppKit
import PaddockKit
import SwiftUI

/// The first-run window: four steps, numbered on the left, content on the
/// right, Back / Continue bottom right, Quit bottom left on step 1 only.
/// It never asks for a permission.
@MainActor
struct OnboardingView: View {
    @Environment(LauncherStore.self) private var store
    @Environment(UIRouter.self) private var router
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow
    @State private var step = 1
    @State private var doctor: DoctorJSON?
    @State private var checking = false
    @State private var useIPSW = false
    @State private var ipsw: String?
    @State private var showCancel = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private let titles: [LocalizedStringKey] = ["What paddock does", "Check this Mac", "Build the base image", "Building"] // localized

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 0) {
                stepList
                    .frame(width: 190)
                    .padding(.top, 24)
                Divider()
                if step == 4 {
                    // A grouped Form scrolls by itself.
                    buildingStep
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                        .padding(8)
                } else {
                    ScrollView {
                        content
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(24)
                    }
                }
            }
            Divider()
            footer.padding(16)
        }
        .onAppear {
            AppContainer.openWindow = openWindow
            if case .building = store.golden { step = 4 }
            if case .failed = store.golden { step = 4 }
        }
        .task(id: step) { if step == 2 { await check() } }
        .confirmationDialog(Text(BuildCopy.cancelQuestion), isPresented: $showCancel) {
            Button("Keep Building", role: .cancel) {}
            Button("Stop", role: .destructive) { store.cancelBuild() }
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Welcome to paddock"))
    }

    // MARK: Steps list

    private var stepList: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(1...4, id: \.self) { index in
                HStack(spacing: 8) {
                    Image(systemName: OnboardingSteps.symbol(index, current: step))
                        .foregroundStyle(index < step ? Color.green : index == step ? Color.accentColor : Color.secondary)
                        .font(.body.weight(index == step ? .semibold : .regular))
                        .contentTransition(.symbolEffect(.replace))
                        .accessibilityLabel(index < step ? Text("Done") : index == step ? Text("Current step") : Text("Not started"))
                    Text(titles[index - 1])
                        .fontWeight(index == step ? .semibold : .regular)
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("onboarding-step-" + String(index))
            }
            Spacer()
        }
        .padding(.horizontal, 16)
        .animation(reduceMotion ? nil : .default, value: step)
    }

    // MARK: Content

    @ViewBuilder private var content: some View {
        switch step {
        case 1: whatStep
        case 2: checkStep
        default: buildStep
        }
    }

    private var whatStep: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 10) {
                Image(nsImage: NSApp.applicationIconImage)
                    .resizable()
                    .interpolation(.high)
                    .frame(width: 64, height: 64)
                    .accessibilityHidden(true)
                Text("Run apps in their own sandbox").font(.title2.bold())
            }
            featureRow("shield.lefthalf.filled") {
                Text("Each app runs inside a separate macOS virtual machine on this Mac. Its windows appear on your desktop like any other app.")
            }
            featureRow("folder") { Text("The app only sees the folders you share with it.") }
            featureRow("network") { Text("The first time an app wants to reach a new server, paddock asks you.") }
            Text("paddock needs a base image of macOS, about 25 GB, built once. Apps start from a copy of it in about half a minute.")
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// A row of step 1: a 24 pt accent symbol, then one sentence.
    private func featureRow<Content: View>(_ symbol: String, @ViewBuilder text: () -> Content) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: symbol)
                .font(.system(size: 24))
                .symbolRenderingMode(.hierarchical)
                .foregroundStyle(Color.accentColor)
                .frame(width: 32)
                .accessibilityHidden(true)
            text()
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var rows: [PrereqRow] {
        var verdict: VersionGate.Verdict?
        if case .found(_, let v) = store.binary { verdict = v }
        return Prerequisites.rows(doctor: doctor, binary: verdict, binaryMissing: store.binary == .missing)
    }

    /// Step 2 as a checklist: each check, its result under it, and the
    /// result symbol at the end of the row.
    private var checkStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Check this Mac").font(.title2.bold())
            ForEach(rows) { row in
                HStack(alignment: .top, spacing: 10) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(row.title).font(.headline)
                        Text(row.result).fixedSize(horizontal: false, vertical: true)
                        if let command = row.command { CopyField(command: command) }
                        HStack { ForEach(row.actions, id: \.self) { action in actionButton(action) } }
                    }
                    Spacer(minLength: 8)
                    if row.status == .checking {
                        ProgressView()
                            .controlSize(.small)
                            .accessibilityLabel(Text(row.accessibilityLabel))
                    } else {
                        StatusSymbol(name: row.symbol, tint: row.tint, label: row.accessibilityLabel)
                            .font(.title3)
                            .transition(.opacity)
                    }
                }
                .padding(.vertical, 2)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("prereq-" + row.kind.rawValue)
                if row.id != rows.last?.id { Divider() }
            }
        }
        .animation(reduceMotion ? nil : .default, value: rows)
    }

    @ViewBuilder
    private func actionButton(_ action: PrereqRow.Action) -> some View {
        switch action {
        case .quit: Button("Quit") { NSApp.terminate(nil) }
        case .updateMacOS: Button("Update macOS in System Settings") { SystemLinks.open(SystemLinks.softwareUpdate) }
        case .copyInstall, .copyUpgrade: EmptyView()
        case .manageStorage: Button("Manage Storage…") { SystemLinks.manageStorage() }
        case .checkAgain:
            Button("Check again") { Task { await check(again: true) } }
                .disabled(checking)
                .accessibilityIdentifier("check-again")
        }
    }

    private var buildStep: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Build the base image").font(.title2.bold())
            Text(BuildCopy.body).fixedSize(horizontal: false, vertical: true)
            if let doctor {
                SpaceGauge(free: doctor.freeBytes, need: doctor.needBytes)
            }
            LabeledContent("Keep in") {
                Text(verbatim: Format.tildePath(store.golden.info?.dir ?? defaultDir, home: NSHomeDirectory()))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }
            Text("paddock keeps the base image in this folder. It needs about 40 GB free on this disk.")
                .font(.callout)
                .labelInk()
            Toggle("Use a restore image I already have…", isOn: $useIPSW)
                .onChange(of: useIPSW) { _, on in
                    if on, ipsw == nil { ipsw = Pickers.restoreImage() }
                    if ipsw == nil { useIPSW = false }
                }
            if useIPSW, let ipsw {
                Text(verbatim: ipsw).lineLimit(1).truncationMode(.middle)
            }
            CommandEcho(args: LibraryArgs.setup(ipsw: useIPSW ? ipsw : nil, resume: false))
        }
    }

    private var defaultDir: String {
        NSHomeDirectory() + "/Library/Application Support/paddock/macvm"
    }

    @ViewBuilder private var buildingStep: some View {
        switch store.golden {
        case .ready, .needsUpdate:
            VStack(alignment: .leading, spacing: 14) {
                Label { Text("Base image ready").font(.title2.bold()) } icon: {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(.green).accessibilityHidden(true)
                }
                Text("Add an app from your Applications folder to start.")
            }
            .padding(16)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("onboarding-done")
        case .failed(let failure):
            Form { BuildFailureSection(failure: failure) }.formStyle(.grouped)
        case .building(let progress):
            Form { BuildProgressSection(progress: progress, cancel: { showCancel = true }) }.formStyle(.grouped)
        case .none:
            ProgressView().accessibilityLabel(Text("Loading"))
        }
    }

    // MARK: Footer

    private var footer: some View {
        HStack {
            if step == 1 {
                Button("Quit") { NSApp.terminate(nil) }
            }
            Spacer()
            if step > 1, step < 4 {
                Button("Back") { step -= 1 }
            }
            switch step {
            case 1, 2:
                Button("Continue") { step += 1 }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(step == 2 && !Prerequisites.canContinue(rows))
                    .accessibilityIdentifier("onboarding-continue")
            case 3:
                Button("Build") {
                    store.build(ipsw: useIPSW ? ipsw : nil)
                    step = 4
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .accessibilityIdentifier("onboarding-build")
            default:
                if store.golden.isReady {
                    Button("Open paddock") { finish(addApp: false) }
                    Button("Add App…") { finish(addApp: true) }
                        .buttonStyle(.borderedProminent)
                        .keyboardShortcut(.defaultAction)
                        .accessibilityIdentifier("onboarding-add-app")
                }
            }
        }
    }

    private func finish(addApp: Bool) {
        openWindow(id: WindowID.main)
        if addApp { router.sheet = .addApp(path: nil) }
        dismissWindow(id: WindowID.onboarding)
    }

    private func check(again: Bool = false) async {
        checking = true
        defer { checking = false }
        if again { await AppContainer.checkAgain() }
        if store.binary.isUsable, let service = store.service {
            doctor = try? await service.doctor()
        } else {
            doctor = LocalDoctor.make()
        }
    }
}

/// The symbols of the onboarding steps: a check when done, a filled ring
/// for the current step, an empty ring for the rest.
enum OnboardingSteps {
    static func symbol(_ index: Int, current: Int) -> String {
        if index < current { return "checkmark.circle.fill" }
        if index == current { return "circle.inset.filled" }
        return "circle"
    }
}

/// The free space on this disk against what the build needs, as a bar with
/// the two sizes. Orange when the space is short.
@MainActor
struct SpaceGauge: View {
    let free: Int64
    let need: Int64

    var body: some View {
        let text = UIText("Needs about %@, %@ free", Format.gb(need), Format.gb(free))
        VStack(alignment: .leading, spacing: 4) {
            Gauge(value: Double(min(need, max(free, 1))), in: 0...Double(max(free, 1))) {
                Text("Space")
            }
            .gaugeStyle(.accessoryLinearCapacity)
            .tint(free >= need ? Color.blue : Color.orange)
            .labelsHidden()
            .accessibilityLabel(Text("Space"))
            .accessibilityValue(Text(text))
            Text(text)
                .font(.callout)
                .labelInk()
                .monospacedDigit()
                .accessibilityHidden(true)
        }
        .frame(maxWidth: 360, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("space-gauge")
    }
}

/// The checks the launcher can do without paddock, for the checklist when
/// paddock is missing: chip, macOS version, free space and memory.
enum LocalDoctor {
    static let needBytes: Int64 = 40_000_000_000

    static func make() -> DoctorJSON? {
        var arm: Int32 = 0
        var size = MemoryLayout<Int32>.size
        let isApple = sysctlbyname("hw.optional.arm64", &arm, &size, nil, 0) == 0 && arm == 1
        let os = ProcessInfo.processInfo.operatingSystemVersion
        let home = URL(fileURLWithPath: NSHomeDirectory())
        let free = (try? home.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]))?
            .volumeAvailableCapacityForImportantUsage ?? 0
        let json = """
        {"v":1,"chip":"\(isApple ? "apple" : "intel")","macos":"\(os.majorVersion).\(os.minorVersion)",
         "macos_ok":\(os.majorVersion >= 14),"free_bytes":\(free),"need_bytes":\(needBytes),
         "memory_bytes":\(ProcessInfo.processInfo.physicalMemory)}
        """
        return try? PaddockJSON.decoder.decode(DoctorJSON.self, from: Data(json.utf8))
    }
}
