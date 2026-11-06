import Foundation

/// The steps of a base image build, in order.
public enum BuildStep: String, Equatable, Sendable, CaseIterable {
    case download, install, guided, seal

    public init?(json: String) {
        switch json {
        case "download": self = .download
        case "install": self = .install
        case "guided", "setup", "first_boot": self = .guided
        case "seal": self = .seal
        default: return nil
        }
    }

    public var title: UIText {
        switch self {
        case .download: return UIText("Download macOS")
        case .install: return UIText("Install")
        case .guided: return UIText("First boot and setup")
        case .seal: return UIText("Seal")
        }
    }
}

/// A known build failure and the next step the launcher offers for it.
public enum BuildFailureAction: Equatable, Sendable {
    case manageStorage
    case tryAgain
    case chooseRestoreImage

    public var buttonTitle: UIText {
        switch self {
        case .manageStorage: return UIText("Manage Storage…")
        case .tryAgain: return UIText("Try Again")
        case .chooseRestoreImage: return UIText("Choose another restore image…")
        }
    }
}

public struct BuildFailure: Equatable, Sendable {
    public var step: BuildStep?
    public var code: String
    /// paddock's reason line, verbatim.
    public var reason: String

    public init(step: BuildStep?, code: String, reason: String) {
        self.step = step
        self.code = code
        self.reason = reason
    }

    /// The primary button for the failure code.
    public var primary: BuildFailureAction {
        switch code {
        case "no_space", "low_disk": return .manageStorage
        case "unsupported_image": return .chooseRestoreImage
        default: return .tryAgain
        }
    }

    /// A next-step line under paddock's reason, when the code has one.
    public var hint: UIText? {
        switch code {
        case "download_interrupted": return UIText("Check your connection, then Try Again.")
        default: return nil
        }
    }
}

public struct GoldenInfo: Equatable, Sendable {
    public var version: String
    public var built: Date?
    public var sizeBytes: Int64
    public var dir: String?
    public var appsUsing: Int

    public init(version: String, built: Date?, sizeBytes: Int64, dir: String?, appsUsing: Int) {
        self.version = version
        self.built = built
        self.sizeBytes = sizeBytes
        self.dir = dir
        self.appsUsing = appsUsing
    }
}

public enum GoldenState: Equatable, Sendable {
    case none
    case building(BuildProgress)
    case failed(BuildFailure)
    case ready(GoldenInfo)
    case needsUpdate(GoldenInfo, hostVersion: String)

    public init(json: GoldenJSON) {
        let info = GoldenInfo(version: json.version ?? "", built: json.built, sizeBytes: json.sizeBytes ?? 0,
                              dir: json.dir, appsUsing: json.appsUsing ?? 0)
        switch json.state {
        case "building":
            var progress = BuildProgress()
            if let setup = json.setup { progress.apply(setup) }
            self = .building(progress)
        case "failed":
            let failure = json.setup?.failed ?? SetupFailureJSON(code: "unknown", reason: "")
            self = .failed(BuildFailure(step: json.setup.flatMap { BuildStep(json: $0.step) },
                                        code: failure.code, reason: failure.reason))
        case "ready": self = .ready(info)
        case "needs_update": self = .needsUpdate(info, hostVersion: json.hostVersion ?? "")
        default: self = .none
        }
    }

    public var isReady: Bool {
        switch self {
        case .ready, .needsUpdate: return true
        default: return false
        }
    }

    public var info: GoldenInfo? {
        switch self {
        case .ready(let info), .needsUpdate(let info, _): return info
        default: return nil
        }
    }

    /// The disabled header row of the menu bar extra.
    public var menuHeader: UIText {
        switch self {
        case .none, .failed: return UIText("No base image")
        case .building(let progress):
            if let percent = progress.percent { return UIText("Building base image · %@", "\(percent) %") }
            return UIText("Building base image")
        case .ready, .needsUpdate: return UIText("Base image ready")
        }
    }
}

/// The state of a running build, fed by `macvm setup --json` lines and
/// `setup` events. Times are stream times (the `time` field of each line),
/// so a replayed stream gives the same texts as a live one.
public struct BuildProgress: Equatable, Sendable {
    public var step: BuildStep = .download
    public var bytes: Int64 = 0
    public var total: Int64 = 0
    public var rate: Double = 0
    public var paused = false
    public var done = false
    /// The stream time of the first line since the download started or resumed.
    public var rateSince: Date?
    public var lastTime: Date?

    /// The time of stable rate the launcher waits for before it shows an estimate.
    public static let estimateAfter: TimeInterval = 30

    public init() {}

    public mutating func apply(_ line: SetupJSON) {
        if line.step == "done" {
            done = true
            return
        }
        let newStep = BuildStep(json: line.step) ?? step
        let wasPaused = paused
        paused = line.paused ?? false
        let resumed = wasPaused && !paused
        if paused {
            rateSince = nil
        } else if newStep != step || resumed || rateSince == nil {
            rateSince = line.time
        }
        step = newStep
        if let b = line.bytes { bytes = b }
        if let t = line.total { total = t }
        if let r = line.rate { rate = r }
        lastTime = line.time ?? lastTime
    }

    public var percent: Int? {
        guard step == .download, total > 0 else { return nil }
        return Int((Double(bytes) / Double(total) * 100).rounded(.down))
    }

    public var fraction: Double? {
        guard step == .download, total > 0 else { return nil }
        return min(1, Double(bytes) / Double(total))
    }

    /// True once the rate was stable for 30 s of stream time.
    public var showsEstimate: Bool {
        guard step == .download, !paused, rate > 0, let since = rateSince, let last = lastTime else { return false }
        return last.timeIntervalSince(since) >= BuildProgress.estimateAfter
    }

    /// `6.2 of 13.1 GB · 48 % · 9 min left`, or `Paused · 6.2 of 13.1 GB`.
    public var downloadLine: UIText? {
        guard step == .download, total > 0 else { return nil }
        let have = Format.gbNumber(bytes)
        let all = Format.gbNumber(total)
        if paused { return UIText("Paused · %@ of %@ GB", have, all) }
        let pct = "\(percent ?? 0) %"
        if showsEstimate {
            let left = Format.timeLeft(seconds: Double(total - bytes) / rate).english
            return UIText("%@ of %@ GB · %@ · %@", have, all, pct, left)
        }
        return UIText("%@ of %@ GB · %@", have, all, pct)
    }

    /// The status of one step in the steps list.
    public func status(of other: BuildStep) -> StepStatus {
        if done { return .done }
        let order = BuildStep.allCases
        guard let mine = order.firstIndex(of: step), let theirs = order.firstIndex(of: other) else { return .pending }
        if theirs < mine { return .done }
        if theirs > mine { return .pending }
        return paused ? .paused : .current
    }

    /// The line under the current step.
    public func detail(of other: BuildStep) -> UIText? {
        guard status(of: other) == .current || status(of: other) == .paused else { return nil }
        switch other {
        case .download: return downloadLine
        case .install: return UIText("Installing macOS, about 8 min")
        case .guided: return UIText("Waiting for the setup window…")
        case .seal: return UIText("Sealing the base image")
        }
    }
}

public enum StepStatus: Equatable, Sendable {
    case done, current, paused, pending, failed

    public var symbol: String {
        switch self {
        case .done: return "checkmark.circle.fill"
        case .current: return "circle.fill"
        case .paused: return "pause.circle"
        case .pending: return "circle"
        case .failed: return "exclamationmark.triangle.fill"
        }
    }

    public var tint: SymbolTint {
        switch self {
        case .done: return .green
        case .failed: return .orange
        default: return .secondary
        }
    }

    public var accessibilityLabel: UIText {
        switch self {
        case .done: return UIText("Done")
        case .current: return UIText("In progress")
        case .paused: return UIText("Paused")
        case .pending: return UIText("Not started")
        case .failed: return UIText("Failed")
        }
    }
}

/// Fixed texts of the build screens.
public enum BuildCopy {
    public static let closeNote = UIText("You can close this window; the build continues. Quitting paddock for Mac pauses it.")
    public static let body = UIText("paddock downloads macOS from Apple (about 13 GB), installs it into an image (about 25 GB on disk) and seals it. This takes 10 to 30 minutes on a fast connection. You can pause, quit and continue later.")
    public static let cancelQuestion = UIText("Stop building the base image? The downloaded part stays, so you can continue later.")
    public static let rebuildQuestion = UIText("Rebuild takes 10 to 30 minutes and 25 GB extra until the old image is replaced. Running apps keep the old image until they stop.")

    public static func readyLine(_ info: GoldenInfo) -> UIText {
        let built = info.built.map { Format.dayMonth($0) } ?? "–"
        return UIText("macOS %@ · built %@ · %@", info.version, built, Format.gb(info.sizeBytes))
    }

    public static func removeQuestion(_ info: GoldenInfo) -> UIText {
        UIText("Remove the base image (%@)? Apps cannot start until you build it again. App data stays.", Format.gb(info.sizeBytes))
    }

    public static func needsUpdate(host: String, image: String) -> UIText {
        UIText("This Mac runs macOS %@; the base image is %@. Apps still work.", host, image)
    }
}
