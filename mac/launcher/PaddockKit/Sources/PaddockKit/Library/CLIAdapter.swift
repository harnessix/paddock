import Foundation

/// Reads paddock's JSON in both shapes: the one paddock 0.2.x prints (see
/// CLIShapes.swift) and the older shape of the launcher fixtures. Each
/// function tries the real shape first when its marker key is there.
public enum LibraryDecode {
    struct Keys: Decodable {
        var profiles: [ProfileJSON]?
        var apps: [AppJSON]?
    }

    /// `app list --json`: `profiles` (paddock) or `apps` (older fixtures).
    public static func apps(_ data: Data, now: Date = Date()) throws -> [AppJSON] {
        let keys = try PaddockJSON.decoder.decode(Keys.self, from: data)
        if let profiles = keys.profiles { return profiles.map { AppJSON(profile: $0, now: now) } }
        if let apps = keys.apps { return apps }
        throw CommandError.badJSON(output: String(decoding: data.prefix(200), as: UTF8.self))
    }

    /// `macvm status --json`: `dir` and a golden `state` of `missing`,
    /// `sealed` or a build step mark paddock's shape.
    public static func macvmStatus(_ data: Data) throws -> MacvmStatusJSON {
        if let legacy = try? PaddockJSON.decoder.decode(MacvmStatusJSON.self, from: data),
           GoldenStateWord.legacy.contains(legacy.golden.state) {
            return legacy
        }
        let real = try PaddockJSON.decoder.decode(MacvmStatusV1JSON.self, from: data)
        return MacvmStatusJSON(v1: real)
    }

    /// One `setup` line, from `macvm setup --json` or the event stream.
    /// paddock's lines carry `t` and a `failed` string; the older fixture
    /// lines carry `time` and a `failed` object.
    public static func setupLine(_ data: Data) -> SetupJSON? {
        let real = try? PaddockJSON.decoder.decode(SetupLineJSON.self, from: data)
        if let real, real.t != nil { return SetupJSON(line: real) }
        if let legacy = try? PaddockJSON.decoder.decode(SetupJSON.self, from: data) {
            var line = legacy
            line.step = SetupSteps.launcherStep(legacy.step)
            return line
        }
        return real.map { SetupJSON(line: $0) }
    }

    public static func setupLine(_ text: String) -> SetupJSON? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return setupLine(Data(trimmed.utf8))
    }

    /// `app add --dry-run --json`: the report, also for a refusal.
    public static func addReport(_ data: Data) -> AddReportJSON? {
        guard let report = try? PaddockJSON.decoder.decode(AddReportJSON.self, from: data) else { return nil }
        // An older fixture without `refusals`, `space` or `golden` is no report.
        if report.refusals == nil, report.space == nil, report.dryRun == nil, report.error == nil { return nil }
        return report
    }
}

/// The golden state words of the older launcher shape.
enum GoldenStateWord {
    static let legacy: Set<String> = ["none", "building", "failed", "ready", "needs_update"]
}

/// The steps of `macvm setup` and the four steps the launcher shows.
public enum SetupSteps {
    /// paddock's step to the launcher's: `fetch` is the download; `seed` and
    /// `install` the install; `guided`, `trust` and `configure` the first
    /// boot; `seal` the seal; `sealed` done.
    public static func launcherStep(_ step: String) -> String {
        switch step {
        case "fetch", "download": return "download"
        case "seed", "install": return "install"
        case "guided", "trust", "configure", "setup", "first_boot": return "guided"
        case "seal": return "seal"
        case "sealed", "done": return "done"
        default: return step
        }
    }

    /// A failure code from paddock's failure line, for the next step the
    /// launcher offers. The line itself is shown verbatim.
    public static func failureCode(_ line: String) -> String {
        let text = line.lowercased()
        if text.contains("free") || text.contains("space") || text.contains("disk is full") { return "no_space" }
        if text.contains("not supported") || text.contains("unsupported") || text.contains("cannot run this") {
            return "unsupported_image"
        }
        if text.contains("download") || text.contains("network") || text.contains("connection")
            || text.contains("timed out") || text.contains("offline") {
            return "download_interrupted"
        }
        return "failed"
    }
}

extension SetupJSON {
    /// paddock's setup line in the launcher's shape.
    public init(line: SetupLineJSON) {
        var failure: SetupFailureJSON?
        if let failed = line.failed, !failed.isEmpty {
            failure = SetupFailureJSON(code: SetupSteps.failureCode(failed), reason: failed)
        } else if line.stopped == true {
            failure = SetupFailureJSON(code: "stopped", reason: LibraryCopy.setupStopped.english)
        }
        self.init(step: SetupSteps.launcherStep(line.step), bytes: line.bytes, total: line.total, rate: line.rate,
                  paused: line.paused, time: line.t, failed: failure)
    }
}

extension MacvmStatusJSON {
    /// paddock's `macvm status --json` in the launcher's shape.
    public init(v1: MacvmStatusV1JSON) {
        self.init(v: v1.v, golden: GoldenJSON(v1: v1), disk: nil, logDir: nil)
    }
}

extension GoldenJSON {
    public init(v1 status: MacvmStatusV1JSON) {
        let golden = status.golden
        let appsUsing = Set((status.profiles ?? []).map(\.app)).count
        let built = golden.sealed.flatMap { ISO8601DateFormatter().date(from: $0) }
        let last = status.setup?.last.map { SetupJSON(line: $0) }
        let state: String
        var setup = last
        switch golden.state {
        case "sealed":
            state = GoldenVersion.needsUpdate(host: status.hostVersion, image: golden.version) ? "needs_update" : "ready"
            setup = nil
        case "missing":
            // A first build that stopped early leaves no golden but a manifest.
            state = status.setup?.running == true ? "building" : "none"
        case "unreadable":
            state = "failed"
            setup = SetupJSON(step: "download", failed: SetupFailureJSON(code: "unreadable", reason: golden.error ?? ""))
        default:
            if status.setup?.running == true {
                state = "building"
            } else if let failed = golden.offlineFailed ?? last?.failed?.reason, !failed.isEmpty {
                state = "failed"
                setup = SetupJSON(step: SetupSteps.launcherStep(golden.state),
                                  failed: SetupFailureJSON(code: SetupSteps.failureCode(failed), reason: failed))
            } else if status.setup?.paused == true || status.paused == true {
                state = "building"
                var paused = last ?? SetupJSON(step: SetupSteps.launcherStep(golden.state))
                paused.paused = true
                setup = paused
            } else {
                // The build stopped half way, for example when the launcher quit.
                state = "failed"
                setup = SetupJSON(step: SetupSteps.launcherStep(golden.state),
                                  failed: SetupFailureJSON(code: "stopped", reason: LibraryCopy.buildStopped.english))
            }
        }
        self.init(state: state, version: golden.version, built: built, sizeBytes: golden.bytes, dir: status.dir,
                  hostVersion: status.hostVersion, appsUsing: appsUsing, logPath: nil, setup: setup)
    }
}

/// Compares the macOS of this Mac with the golden's.
public enum GoldenVersion {
    /// `50.1 (99A101)` -> [50, 1].
    public static func numbers(_ text: String?) -> [Int] {
        guard let text else { return [] }
        let word = text.split(separator: " ").first.map(String.init) ?? ""
        return word.split(separator: ".").compactMap { Int($0) }
    }

    /// True when the Mac runs a newer macOS than the golden.
    public static func needsUpdate(host: String?, image: String?) -> Bool {
        let h = numbers(host)
        let i = numbers(image)
        guard !h.isEmpty, !i.isEmpty else { return false }
        for index in 0..<max(h.count, i.count) {
            let a = index < h.count ? h[index] : 0
            let b = index < i.count ? i[index] : 0
            if a != b { return a > b }
        }
        return false
    }
}

extension AppJSON {
    /// One profile instance of `app list --json` in the launcher's shape.
    public init(profile: ProfileJSON, now: Date = Date()) {
        name = profile.name
        home = profile.home
        displayName = profile.displayName
        version = profile.version
        bytes = profile.bytes
        launcher = profile.launcher
        bundlePath = profile.bundle
        let status = profile.status
        let word: String
        if let blocked = profile.blocked, !blocked.isEmpty {
            word = "blocked"
            reason = blocked
            blockCode = ProfileState.blockCode(blocked)
        } else {
            let mapped = ProfileState.state(of: status)
            word = mapped.state
            reason = mapped.reason
        }
        state = word
        suspendReason = status?.suspendReason
        nextStart = status?.nextStart.map { NextStartCopy.text($0).english }
        windows = status?.windows ?? status?.app?.windows
        if let uptime = status?.uptimeS, word == "running" { since = now.addingTimeInterval(-Double(uptime)) }
        if let mode = status?.network?.mode, !mode.isEmpty { network = mode }
        golden = status?.golden
        if let host = status?.host, let data = host.dataBytes, let saved = host.stateBytes, let all = host.profileBytes {
            // `App copy` here is the rest of the profile: the system clone and
            // the app copy, which paddock does not report apart.
            sizes = AppSizesJSON(appCopy: max(0, all - data - saved), data: data, savedState: saved)
        }
    }
}

/// paddock's session state words to the launcher's.
public enum ProfileState {
    public static func state(of status: ProfileStatusJSON?) -> (state: String, reason: String?) {
        guard let status else { return ("stopped", nil) }
        switch status.state {
        case "running": return ("running", nil)
        case "suspended": return ("suspended", status.suspendReason)
        case "stopped":
            switch status.lastEnd {
            case "quit with save"?: return ("saved", nil)
            case "crashed"?: return ("error", status.lastEnd)
            default: return ("stopped", nil)
            }
        default:
            return ("stopped", nil)
        }
    }

    /// A block code from paddock's line, for the next step in the window.
    public static func blockCode(_ line: String) -> String? {
        let text = line.lowercased()
        if text.contains("app store") { return "app_store" }
        if text.contains("macvm setup") || text.contains("golden") { return "no_golden" }
        if text.contains("not found") || text.contains("no such") || text.contains("missing") { return "bundle_missing" }
        return nil
    }
}

/// The `next_start` of paddock in plain words.
public enum NextStartCopy {
    public static func text(_ raw: String) -> UIText {
        if raw == "warm" { return UIText("Warm, about 6 s") }
        if raw == "cold" { return UIText("Cold, about 40 s") }
        if raw.hasPrefix("cold: ") { return UIText("Cold, about 40 s: %@", String(raw.dropFirst("cold: ".count))) }
        if raw.hasPrefix("refused: ") { return UIText("Cannot start: %@", String(raw.dropFirst("refused: ".count))) }
        return UIText(key: "%@", args: [raw])
    }
}

extension PrecheckJSON {
    /// The add report in the shape of the older precheck.
    public init(report: AddReportJSON) {
        v = report.v
        ok = report.ok ?? false
        name = report.name ?? report.displayName ?? ""
        version = report.version
        sizeBytes = report.bytes
        appStore = report.appStore
        switch report.golden {
        case "sealed"?: golden = "ready"
        case "missing"?, "unreadable"?: golden = "none"
        case nil: golden = nil
        default: golden = "building"
        }
        needBytes = report.space?.need
        freeBytes = report.space?.free
        memoryOptionsGb = AddReview.memoryOptions(report.memory)
        message = report.refusals?.first?.text ?? report.error
    }
}
