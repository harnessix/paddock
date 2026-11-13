import XCTest
@testable import PaddockKit

/// Base image build progress and failures, app states, the error table.
@MainActor
final class StateTests: XCTestCase {
    // Every base fixture decodes with the launcher's models.
    func testAllFixturesDecode() throws {
        let names = try FileManager.default.contentsOfDirectory(atPath: Fixtures.root.appendingPathComponent("base").path)
        for name in names where name.hasSuffix(".json") {
            let data = try Fixtures.data(name)
            let decoder = PaddockJSON.decoder
            let ok: Bool
            switch name {
            case let n where n.hasPrefix("status-v1-"): ok = (try? LibraryDecode.macvmStatus(data)) != nil
            case let n where n.hasPrefix("status-") && !n.contains("sample-notes") && !n.contains("sample-mail"):
                ok = (try? decoder.decode(MacvmStatusJSON.self, from: data)) != nil
            case let n where n.hasPrefix("profiles"): ok = (try? decoder.decode(ProfileListJSON.self, from: data)) != nil
            case let n where n.hasPrefix("du"): ok = (try? decoder.decode(DuReportJSON.self, from: data)) != nil
            // Some protocol answers are wrong on purpose. LauncherProtocolTests checks each one.
            case let n where n.hasPrefix("protocol-"): ok = true
            case let n where n.hasPrefix("gc-"): ok = (try? decoder.decode(GcReportJSON.self, from: data)) != nil
            case let n where n.hasPrefix("pause-"): ok = (try? decoder.decode(SetupControlJSON.self, from: data)) != nil
            case let n where n.hasPrefix("remove-report"): ok = (try? decoder.decode(RemoveReportJSON.self, from: data)) != nil
            case let n where n.hasPrefix("add-re"): ok = LibraryDecode.addReport(data) != nil
            case let n where n.hasPrefix("status-"): ok = (try? decoder.decode(AppStatusJSON.self, from: data)) != nil
            case let n where n.hasPrefix("apps"): ok = (try? decoder.decode(AppListJSON.self, from: data)) != nil
            case let n where n.hasPrefix("approvals"): ok = (try? decoder.decode(ApprovalsJSON.self, from: data)) != nil
            case let n where n.hasPrefix("doctor"): ok = (try? decoder.decode(DoctorJSON.self, from: data)) != nil
            case let n where n.hasPrefix("precheck"): ok = (try? decoder.decode(PrecheckJSON.self, from: data)) != nil
            case let n where n.hasPrefix("rules-"): ok = (try? decoder.decode(NetRulesJSON.self, from: data)) != nil
            case let n where n.hasPrefix("connections"): ok = (try? decoder.decode(ConnectionsJSON.self, from: data)) != nil
            case let n where n.hasPrefix("clipboard-"): ok = (try? decoder.decode(ClipboardStateJSON.self, from: data)) != nil
            default: ok = (try? decoder.decode(ResultJSON.self, from: data)) != nil
            }
            XCTAssertTrue(ok, "\(name) does not decode")
        }
        for name in names where name.hasSuffix(".jsonl") {
            for line in try Fixtures.stream(name) {
                XCTAssertNotNil(PaddockEvent.decode(line: line), "\(name): \(line)")
            }
        }
    }

    // paddock's notice lines decode and reach the hook that posts them:
    // paddock does not post them itself while the launcher follows.
    func testNoticeEventsReachTheLauncher() throws {
        XCTAssertTrue(PaddockArgs.events.contains("--notify"))
        let lines = try ReplayRunner.streamLines(Fixtures.core("launcher/notice.jsonl"))
        let notices = lines.compactMap { line -> NoticeEvent? in
            if case .notice(let n)? = PaddockEvent.decode(line: line) { return n }
            return nil
        }
        XCTAssertEqual(notices.count, lines.count, "every notice line of paddock decodes")
        XCTAssertEqual(notices[0].what, "approval")
        XCTAssertEqual(notices[0].action, "r12")
        XCTAssertEqual(notices[0].critical, true)
        XCTAssertNil(notices[1].action)
        let store = LauncherStore(service: nil)
        var shown: [NoticeEvent] = []
        store.onNotice = { shown.append($0) }
        for notice in notices { store.apply(.notice(notice)) }
        XCTAssertEqual(shown, notices)
    }

    func testProducerApprovalSnapshotKeepsPresentationAndNoticeIDs() throws {
        let approvals = try Fixtures.decode(ApprovalsJSON.self, "approvals-paddock-v1.json")
        XCTAssertEqual(approvals.questions.map(\.id), ["r-shown-g1"])
        XCTAssertEqual(approvals.questions.first?.app, "sample-notes")
        XCTAssertEqual(approvals.questions.first?.host, "api.example.com")
        XCTAssertEqual(approvals.questions.first?.port, 443)
        XCTAssertEqual(approvals.paused?.id, "r-notice")
        XCTAssertEqual(approvals.paused?.app, "sample-mail")
        XCTAssertEqual(approvals.paused?.count, 21)
        let legacy = try Fixtures.decode(ApprovalsJSON.self, "approvals-paused.json")
        XCTAssertNil(legacy.paused?.id)
        XCTAssertEqual(legacy.paused?.count, 23)
    }

    // MARK: Build progress

    func testBuildProgressReplay() throws {
        var progress = BuildProgress()
        var lines: [SetupJSON] = []
        for line in try Fixtures.stream("setup-stream.jsonl") {
            guard case .setup(let s)? = PaddockEvent.decode(line: line) else { return XCTFail(line) }
            lines.append(s)
        }
        // 0 s: download is the current step, no estimate yet.
        progress.apply(lines[0])
        XCTAssertEqual(progress.status(of: .download), .current)
        XCTAssertEqual(progress.status(of: .install), .pending)
        XCTAssertEqual(progress.status(of: .download).symbol, "circle.fill")
        XCTAssertEqual(progress.status(of: .install).symbol, "circle")
        XCTAssertEqual(progress.downloadLine?.english, "0.0 of 13.1 GB · 0 %")
        // 29 s of stream time: still no estimate.
        for i in 1...3 { progress.apply(lines[i]) }
        XCTAssertFalse(progress.showsEstimate)
        XCTAssertEqual(progress.downloadLine?.english, "0.9 of 13.1 GB · 6 %")
        // 31 s: the estimate shows. (13.1 - 0.93) GB at 30 MB/s = 6.8 min.
        progress.apply(lines[4])
        XCTAssertTrue(progress.showsEstimate)
        XCTAssertEqual(progress.downloadLine?.english, "0.9 of 13.1 GB · 7 % · 7 min left")
        progress.apply(lines[5])
        // Pause.
        progress.apply(lines[6])
        XCTAssertEqual(progress.downloadLine?.english, "Paused · 1.2 of 13.1 GB")
        XCTAssertEqual(progress.status(of: .download), .paused)
        XCTAssertEqual(progress.status(of: .download).symbol, "pause.circle")
        // Resume: the 30 s wait starts again.
        progress.apply(lines[7])
        XCTAssertFalse(progress.showsEstimate)
        XCTAssertEqual(progress.downloadLine?.english, "1.2 of 13.1 GB · 9 %")
        progress.apply(lines[8])
        XCTAssertEqual(progress.percent, 100)
        // Install: download done, install current and indeterminate.
        progress.apply(lines[9])
        XCTAssertEqual(progress.status(of: .download), .done)
        XCTAssertEqual(progress.status(of: .download).symbol, "checkmark.circle.fill")
        XCTAssertNil(progress.fraction)
        XCTAssertEqual(progress.detail(of: .install)?.english, "Installing macOS, about 8 min")
        progress.apply(lines[10])
        XCTAssertEqual(progress.detail(of: .guided)?.english, "Waiting for the setup window…")
        progress.apply(lines[11])
        XCTAssertEqual(progress.status(of: .seal), .current)
        XCTAssertEqual(progress.status(of: .guided), .done)
        XCTAssertEqual(BuildCopy.closeNote.english,
                       "You can close this window; the build continues. Quitting paddock for Mac pauses it.")
    }

    func testStoreFollowsSetupEvents() async throws {
        let h = try await Harness("fresh")
        XCTAssertEqual(h.store.golden, .none)
        try await h.play("setup-stream.jsonl")
        await waitUntil { if case .building(let p) = h.store.golden { return p.step == .seal } else { return false } }
        XCTAssertEqual(h.store.golden.menuHeader.english, "Building base image")
    }

    func testPausedStateAfterRelaunch() async throws {
        let h = try await Harness("paused")
        guard case .building(let progress) = h.store.golden else { return XCTFail("not building") }
        XCTAssertTrue(progress.paused)
        XCTAssertEqual(progress.downloadLine?.english, "Paused · 6.2 of 13.1 GB")
    }

    // MARK: Build failures

    func testFailureReasonsMapToTheirButton() async throws {
        let cases: [(String, String)] = [
            ("failed-no-space", "Manage Storage…"),
            ("failed-download", "Try Again"),
            ("failed-image", "Choose another restore image…"),
        ]
        for (scenario, button) in cases {
            let h = try await Harness(scenario)
            guard case .failed(let failure) = h.store.golden else { return XCTFail(scenario) }
            XCTAssertEqual(failure.primary.buttonTitle.english, button, scenario)
            XCTAssertFalse(failure.reason.isEmpty)
        }
        let download = BuildFailure(step: .download, code: "download_interrupted", reason: "x")
        XCTAssertEqual(download.hint?.english, "Check your connection, then Try Again.")
        XCTAssertEqual(BuildFailure(step: nil, code: "wrong_password", reason: "x").primary, .tryAgain)
    }

    func testSetupFailureEvent() async throws {
        let h = try await Harness("fresh")
        h.events.send(#"{"v":1,"kind":"setup","step":"download","failed":{"code":"no_space","reason":"The disk is full."}}"#)
        await waitUntil { if case .failed = h.store.golden { return true } else { return false } }
        guard case .failed(let f) = h.store.golden else { return }
        XCTAssertEqual(f.reason, "The disk is full.")
        XCTAssertEqual(f.primary, .manageStorage)
    }

    // MARK: App states

    func testAppWalkRowTextSymbolAndLabel() async throws {
        let h = try await Harness("stopped")
        let expected: [(String, String?, String, Bool)] = [
            ("Stopped", "stop.circle", "Stopped", false),
            ("Starting… cloning", nil, "Starting", true),
            ("Starting… booting", nil, "Starting", true),
            ("Starting… copying app", nil, "Starting", true),
            ("Starting… opening", nil, "Starting", true),
            ("Running · 3 windows", "play.circle.fill", "Running", false),
            ("Stopping…", nil, "Stopping", true),
            ("Ready · saved just now", "pause.circle", "Ready", false),
            ("The VM stopped: out of memory", "exclamationmark.triangle.fill", "Error: The VM stopped: out of memory", false),
            ("Build the base image to start this app.", "nosign", "Blocked: Build the base image to start this app.", false),
        ]
        let lines = try Fixtures.stream("app-walk.jsonl")
        XCTAssertEqual(lines.count, expected.count)
        for (line, want) in zip(lines, expected) {
            h.store.apply(PaddockEvent.decode(line: line)!)
            var now = h.clock.now()
            if case .saved(let since?) = h.store.app(named: "Sample Notes")!.state { now = since.addingTimeInterval(5) }
            let p = StatusPresentation.of(h.store.app(named: "Sample Notes")!.state, now: now)
            XCTAssertEqual(p.text.english, want.0)
            XCTAssertEqual(p.symbol, want.1)
            XCTAssertEqual(p.accessibilityLabel.english, want.2)
            XCTAssertFalse(p.accessibilityLabel.english.isEmpty)
            XCTAssertEqual(p.showsProgress, want.3)
        }
    }

    // The status pill: one word per state, a symbol for every word, and
    // only Starting pulses.
    func testPillWordsAndPulse() {
        let now = ISO.date("2026-11-06T10:00:00Z")
        let cases: [(AppState, String, Bool)] = [
            (.stopped, "Stopped", false),
            (.saved(since: now), "Ready", false),
            (.running(since: nil, windows: 3), "Running", false),
            (.starting(.booting), "Starting", true),
            (.suspended(reason: nil), "Suspended", false),
            (.stopping, "Stopping", false),
            (.error(reason: "The VM stopped: out of memory"), "Error", false),
            (.blocked(reason: "Build the base image to start this app.", code: "no_golden"), "Blocked", false),
        ]
        for (state, word, pulses) in cases {
            let p = StatusPresentation.of(state, now: now)
            XCTAssertEqual(p.word.english, word)
            XCTAssertEqual(p.pulses, pulses, word)
            XCTAssertTrue(p.symbol != nil || p.showsProgress, word)
        }
    }

    // The memory gauge reads `6 of 16 GB`; the disk bar has three parts in
    // bar order that add up to the whole.
    func testMemoryGaugeAndDiskParts() throws {
        let json = #"{"name":"Sample Notes","state":"running","memory_bytes":6442450944,"host_memory_bytes":17179869184}"#
        let app = try PaddockJSON.decoder.decode(AppJSON.self, from: Data(json.utf8))
        XCTAssertEqual(OverviewCopy.memory(app)?.english, "6 of 16 GB")
        XCTAssertEqual(OverviewCopy.memoryFraction(app) ?? 0, 0.375, accuracy: 0.001)
        let parts = OverviewCopy.diskParts(AppSizesJSON(appCopy: 2_000_000_000, data: 1_000_000_000, savedState: 1_000_000_000))
        XCTAssertEqual(parts.map(\.kind), [.appCopy, .data, .savedState])
        XCTAssertEqual(parts.map(\.title.english), ["App copy", "Data", "Saved state"])
        XCTAssertEqual(parts.map(\.fraction).reduce(0, +), 1, accuracy: 0.0001)
        XCTAssertEqual(parts[0].fraction, 0.5, accuracy: 0.0001)
        XCTAssertTrue(OverviewCopy.diskParts(AppSizesJSON(appCopy: 0, data: 0, savedState: 0)).isEmpty)
    }

    func testSavedAgeAndTints() {
        let now = ISO.date("2026-11-06T10:00:00Z")
        let saved = AppState.saved(since: now.addingTimeInterval(-2 * 3600))
        XCTAssertEqual(StatusPresentation.of(saved, now: now).text.english, "Ready · saved 2 h ago")
        XCTAssertEqual(StatusPresentation.of(.running(since: nil, windows: 1), now: now).text.english, "Running · 1 window")
        XCTAssertEqual(StatusPresentation.of(.running(since: nil, windows: 1), now: now).tint, .green)
        XCTAssertEqual(StatusPresentation.of(.error(reason: "x"), now: now).tint, .orange)
        XCTAssertEqual(StatusPresentation.of(.blocked(reason: "x", code: nil), now: now).tint, .red)
    }

    // MARK: Errors table

    func testErrorTableStrings() {
        let rows: [(LauncherError, String?, String?, String?)] = [
            (.notInstalled, "paddock is not installed",
             "paddock for Mac needs the paddock command. Install it with Homebrew, then check again.", "Check Again"),
            (.tooOld(have: "0.2.1", need: "0.2.2"), "Update paddock",
             "This version of paddock for Mac needs paddock 0.2.2 or newer. You have 0.2.1.", "Copy Command"),
            (.tooNew(have: "1.0.0"), "Update paddock for Mac", nil, "Copy Command"),
            (.twoRunning(app: "Sample Mail", running: ["Sample Notes", "Notebook"]), "Two apps are already running",
             "macOS allows two sandboxed apps at a time. Stop one to start Sample Mail.", "Stop…"),
            (.lowDiskAtStart(app: "Sample Notes", needBytes: 2_500_000_000, freeBytes: 1_100_000_000),
             "Not enough free space to start Sample Notes", "Starting needs 2.5 GB, 1.1 GB is free.", "Manage Storage…"),
            (.appChanged(app: "Sample Notes"), nil,
             "Sample Notes was updated on this Mac. The next start copies the new version, about 1 min.", nil),
            (.appCopyFailed(app: "Sample Notes", line: "copy failed: disk full"), "Sample Notes runs from the previous copy",
             "copy failed: disk full", "Show Log"),
            (.crashed(app: "Sample Notes", crashRow: "guest kernel panic"), "Sample Notes stopped unexpectedly",
             "Its saved state was not written. The next start is cold.", "Start Again"),
            (.agentAway, nil,
             "paddock lost contact with the app's helper. Menus and clipboard pause until it is back.", nil),
            (.goldenRemoved, nil, "Build the base image to start this app.", "Build…"),
            (.streamLost, "Live updates paused", "Live updates paused", "Refresh"),
        ]
        for (error, title, body, primary) in rows {
            XCTAssertEqual(error.title?.english, title, "\(error)")
            if let body { XCTAssertEqual(error.body?.english, body, "\(error)") }
            XCTAssertEqual(error.primary.title?.english, primary, "\(error)")
        }
        XCTAssertEqual(LauncherError.notInstalled.placement, .alert)
        XCTAssertEqual(LauncherError.appChanged(app: "x").placement, .info)
        XCTAssertEqual(LauncherError.agentAway.placement, .banner)
        XCTAssertEqual(LauncherError.streamLost.placement, .statusLine)
        XCTAssertEqual(LauncherError.goldenRemoved.placement, .row)
        XCTAssertEqual(LauncherError.crashed(app: "x", crashRow: "row").detail, "row")
    }

    func testStartRefusalsShowInPlace() async throws {
        let two = try await Harness("two-vms")
        await two.store.startApp("Sample Notes")
        XCTAssertEqual(two.store.appErrors["Sample Notes"], .twoRunning(app: "Sample Notes", running: ["Sample Notes", "Sample Mail"]))
        let low = try await Harness("low-disk-start")
        await low.store.startApp("Sample Notes")
        XCTAssertEqual(low.store.appErrors["Sample Notes"]?.body?.english, "Starting needs 2.5 GB, 1.1 GB is free.")
    }

    func testCrashEventShowsStartAgain() async throws {
        let h = try await Harness("ready")
        h.store.apply(.session(SessionEvent(app: "Sample Notes", state: "error", phase: nil, since: nil, windows: nil,
                                            reason: "stopped", blockCode: nil, crash: "guest kernel panic",
                                            connected: nil, nextStart: nil)))
        XCTAssertEqual(h.store.appErrors["Sample Notes"]?.primary, .startAgain)
    }
}
