import XCTest
@testable import PaddockKit

/// The library layer: paddock 0.2.x JSON, instances, the add review, the
/// session calls, storage, the drop queue and the launcher's own install.
@MainActor
final class LibraryTests: XCTestCase {
    // MARK: Real shapes

    func testProfileListDecodesInstancesAndStates() throws {
        let now = ISO.date("2026-11-08T10:00:00Z")
        let apps = try LibraryDecode.apps(Fixtures.data("profiles-instances.json"), now: now)
        let items = apps.map(AppItem.init(json:))
        XCTAssertEqual(items.map(\.key), ["sample-notes", "sample-notes--2", "sample-mail", "notebook"])
        XCTAssertEqual(items.map(\.title), ["Sample Notes", "Sample Notes 2", "Sample Mail", "notebook"])
        XCTAssertEqual(items[0].state, .running(since: now.addingTimeInterval(-1200), windows: 3))
        XCTAssertEqual(items[0].json.nextStart, "Warm, about 6 s")
        XCTAssertEqual(items[0].json.network, "ask")
        XCTAssertEqual(items[0].json.sizes, AppSizesJSON(appCopy: 3_700_000_000, data: 3_400_000_000, savedState: 2_000_000_000))
        // paddock gives no status for instance 2: stopped until an event says else.
        XCTAssertEqual(items[1].state, .stopped)
        XCTAssertEqual(items[1].instance, InstanceRef(app: "sample-notes", number: 2))
        XCTAssertEqual(items[2].state, .suspended(reason: "a third session needed the slot"))
        XCTAssertTrue(items[2].state.canResume && items[2].state.canStop && !items[2].state.canStart)
        XCTAssertTrue(items[2].state.holdsSession)
        if case .blocked(let reason, let code) = items[3].state {
            XCTAssertTrue(reason.hasPrefix("The bundle /Applications/Notebook.app is not found."))
            XCTAssertEqual(code, "bundle_missing")
        } else {
            XCTFail("notebook is not blocked: \(items[3].state)")
        }
    }

    func testOlderListShapeStillDecodes() throws {
        let apps = try LibraryDecode.apps(Fixtures.data("apps.json"))
        XCTAssertEqual(apps.map(\.name), ["Sample Notes", "Sample Mail"])
        XCTAssertEqual(AppItem(json: apps[0]).key, "Sample Notes")
    }

    func testSavedAfterQuitWithSave() throws {
        let apps = try LibraryDecode.apps(Fixtures.data("profiles-saved.json"))
        XCTAssertEqual(AppItem(json: apps[0]).state, .saved(since: nil))
    }

    func testSuspendedPresentation() {
        let p = StatusPresentation.of(.suspended(reason: nil), now: Date())
        XCTAssertEqual(p.text.english, "Suspended · resumes in about 4 s")
        XCTAssertEqual(p.accessibilityLabel.english, "Suspended")
        XCTAssertEqual(p.symbol, "pause.circle.fill")
    }

    func testNextStartWords() {
        XCTAssertEqual(NextStartCopy.text("warm").english, "Warm, about 6 s")
        XCTAssertEqual(NextStartCopy.text("cold").english, "Cold, about 40 s")
        XCTAssertEqual(NextStartCopy.text("cold: the golden changed").english, "Cold, about 40 s: the golden changed")
        XCTAssertEqual(NextStartCopy.text("refused: two VMs run").english, "Cannot start: two VMs run")
        XCTAssertEqual(NextStartCopy.text("repair").english, "repair")
    }

    func testMacvmStatusV1() throws {
        let sealed = try LibraryDecode.macvmStatus(Fixtures.data("status-v1-sealed.json"))
        let state = GoldenState(json: sealed.golden)
        guard case .ready(let info) = state else { return XCTFail("\(state)") }
        XCTAssertEqual(info.version, "50.1 (99A101)")
        XCTAssertEqual(info.sizeBytes, 20_000_000_000)
        XCTAssertEqual(info.appsUsing, 1)
        XCTAssertEqual(info.dir, "/Users/demo/Library/Application Support/paddock/macvm")
        XCTAssertNotNil(info.built)

        let newer = try LibraryDecode.macvmStatus(Fixtures.data("status-v1-newer-host.json"))
        guard case .needsUpdate(_, let host) = GoldenState(json: newer.golden) else { return XCTFail() }
        XCTAssertEqual(host, "50.2 (99B102)")

        let fetch = try LibraryDecode.macvmStatus(Fixtures.data("status-v1-fetch.json"))
        guard case .building(let progress) = GoldenState(json: fetch.golden) else { return XCTFail() }
        XCTAssertEqual(progress.step, .download)
        XCTAssertEqual(progress.percent, 47)

        let stopped = try LibraryDecode.macvmStatus(Fixtures.data("status-v1-stopped-build.json"))
        guard case .failed(let failure) = GoldenState(json: stopped.golden) else { return XCTFail() }
        XCTAssertEqual(failure.primary, .tryAgain)
        XCTAssertEqual(failure.step, .install)
        XCTAssertEqual(failure.reason, LibraryCopy.buildStopped.english)

        // The older fixture shape keeps working.
        let legacy = try LibraryDecode.macvmStatus(Fixtures.data("status-ready.json"))
        XCTAssertEqual(legacy.disk?.freeBytes, 184_000_000_000)
    }

    func testGoldenVersionCompare() {
        XCTAssertTrue(GoldenVersion.needsUpdate(host: "50.2 (99B102)", image: "50.1 (99A101)"))
        XCTAssertTrue(GoldenVersion.needsUpdate(host: "51.0", image: "50.7.1"))
        XCTAssertFalse(GoldenVersion.needsUpdate(host: "50.1", image: "50.1 (99A101)"))
        XCTAssertFalse(GoldenVersion.needsUpdate(host: "50.0", image: "50.1"))
        XCTAssertFalse(GoldenVersion.needsUpdate(host: nil, image: "50.1"))
    }

    func testRealSetupLines() throws {
        let lines = try ReplayRunner.streamLines(Fixtures.core("launcher/setup.jsonl"))
        let decoded = lines.compactMap { LibraryDecode.setupLine($0) }
        XCTAssertEqual(decoded.count, lines.count, "every line of paddock's setup fixture decodes")
        XCTAssertEqual(decoded[0].step, "download")
        XCTAssertNotNil(decoded[0].time)
        XCTAssertEqual(decoded[3].paused, true)
        XCTAssertEqual(decoded[5].step, "install")
        XCTAssertEqual(decoded[6].step, "guided")
        XCTAssertEqual(decoded[7].failed?.code, "stopped")
        XCTAssertEqual(decoded[8].failed?.code, "no_space")
        XCTAssertEqual(decoded[8].failed?.reason,
                       "Needs 40.0 GiB on the volume of /Users/u/Library/Application Support/paddock/macvm, 12.1 GiB free.")
        XCTAssertEqual(decoded[9].step, "done")
        // The progress of the download from these lines, in stream time.
        var progress = BuildProgress()
        for line in decoded.prefix(3) { progress.apply(line) }
        XCTAssertEqual(progress.percent, 47)
        XCTAssertTrue(progress.showsEstimate, "four minutes of stable rate")
    }

    func testSetupFailureCodes() {
        XCTAssertEqual(SetupSteps.failureCode("Needs 40.0 GiB on the volume, 12.1 GiB free."), "no_space")
        XCTAssertEqual(SetupSteps.failureCode("The download stopped: the network is offline."), "download_interrupted")
        XCTAssertEqual(SetupSteps.failureCode("This restore image is not supported on this Mac."), "unsupported_image")
        XCTAssertEqual(SetupSteps.failureCode("guided step: the agent did not report"), "failed")
        XCTAssertEqual(SetupSteps.launcherStep("trust"), "guided")
        XCTAssertEqual(SetupSteps.launcherStep("seed"), "install")
        XCTAssertEqual(SetupSteps.launcherStep("sealed"), "done")
    }

    func testRealEventLines() {
        XCTAssertEqual(PaddockEvent.decode(line: #"{"v":1,"t":"2026-11-06T11:30:00Z","kind":"golden","state":"removed","bytes":1}"#),
                       .golden(op: "removed"))
        XCTAssertEqual(PaddockEvent.decode(line: #"{"v":1,"t":"2026-11-06T12:00:30Z","kind":"disk","dir":"/x","free":197568495616}"#),
                       .disk(freeBytes: 197_568_495_616))
        XCTAssertEqual(PaddockEvent.decode(line: #"{"v":1,"t":"2026-11-06T12:20:00Z","kind":"stream","state":"lost"}"#),
                       .stream(state: "lost"))
        guard case .session(let e)? = PaddockEvent.decode(line: #"{"v":1,"t":"2026-11-06T12:30:09Z","kind":"session","app":"sample-notes","home":"instance-2","state":"stopped","text":"quit with save"}"#) else {
            return XCTFail("no session event")
        }
        XCTAssertEqual(e.home, "instance-2")
        XCTAssertEqual(SessionWords.state(e), "saved")
        // The older fixture keys still decode.
        XCTAssertEqual(PaddockEvent.decode(line: #"{"v":1,"kind":"golden","op":"ready"}"#), .golden(op: "ready"))
        XCTAssertEqual(PaddockEvent.decode(line: #"{"v":1,"kind":"disk","free_bytes":5}"#), .disk(freeBytes: 5))
    }

    func testSessionEventReachesTheRightInstance() async throws {
        let h = try await Harness("library")
        XCTAssertEqual(h.store.apps.count, 4)
        h.events.send(#"{"v":1,"t":"2026-11-08T10:01:00Z","kind":"session","app":"sample-notes","home":"instance-2","state":"starting","phase":"booting","text":"the VM runs"}"#)
        await waitUntil { h.store.app(named: "sample-notes--2")?.state == .starting(.booting) }
        XCTAssertTrue(h.store.app(named: "sample-notes")!.state.isRunning, "instance 1 did not change")
        h.events.send(#"{"v":1,"t":"2026-11-08T10:02:00Z","kind":"session","app":"sample-notes","home":"instance-2","state":"stopped","text":"quit with save"}"#)
        await waitUntil {
            if case .saved = h.store.app(named: "sample-notes--2")!.state { return true }
            return false
        }
    }

    func testStreamLostReadsTheStateAgain() async throws {
        let h = try await Harness("library")
        let before = h.calls(["app", "list"]).count
        h.events.send(#"{"v":1,"t":"2026-11-08T10:20:00Z","kind":"stream","state":"lost"}"#)
        await waitUntil { h.calls(["app", "list"]).count > before }
    }

    // MARK: Instances

    func testInstanceNames() {
        let one = InstanceRef(app: "sample-notes", home: "default")
        XCTAssertEqual(one.key, "sample-notes")
        XCTAssertEqual(one.flag, [])
        XCTAssertEqual(one.title(displayName: "Sample Notes"), "Sample Notes")
        let two = InstanceRef(app: "sample-notes", home: "instance-2")
        XCTAssertEqual(two.key, "sample-notes--2")
        XCTAssertEqual(two.home, "instance-2")
        XCTAssertEqual(two.flag, ["--instance", "2"])
        XCTAssertEqual(two.title(displayName: "Sample Notes"), "Sample Notes 2")
        XCTAssertEqual(InstanceRef(key: "sample-notes--2"), two)
        XCTAssertEqual(InstanceRef(key: "sample-notes"), one)
        // Not an instance number: the name stays whole.
        XCTAssertEqual(InstanceRef(key: "odd--name").app, "odd--name")
        XCTAssertEqual(InstanceRef(key: "a--1").app, "a--1")
        XCTAssertEqual(InstanceRef(key: "a--100").app, "a--100")
        XCTAssertEqual(InstanceRef(app: "a", home: "instance-x").number, 1)
        XCTAssertEqual(InstanceRef(app: "a", home: nil).number, 1)
    }

    func testInstanceGroups() throws {
        let apps = try LibraryDecode.apps(Fixtures.data("profiles-instances-new.json")).map(AppItem.init(json:))
        let groups = InstanceGroup.groups(apps)
        XCTAssertEqual(groups.map(\.app), ["sample-notes", "sample-mail", "notebook"])
        XCTAssertTrue(groups[0].isGroup)
        XCTAssertEqual(groups[0].items.map(\.key), ["sample-notes", "sample-notes--2", "sample-notes--3"])
        XCTAssertEqual(groups[0].countText.english, "3 instances")
        XCTAssertEqual(groups[0].title, "Sample Notes")
        XCTAssertFalse(groups[1].isGroup)
    }

    // MARK: Session calls

    func testStartStopAndNewInstanceArguments() async throws {
        let h = try await Harness("library")
        await h.store.startApp("sample-notes--2")
        XCTAssertEqual(h.calls(["app", "run"]).last?.args, ["app", "run", "sample-notes", "--instance", "2"])
        await h.store.startApp("sample-mail")
        XCTAssertEqual(h.calls(["app", "run"]).last?.args, ["app", "run", "sample-mail"], "resume is app run, no --json")
        await h.store.stopApp("sample-notes--2")
        XCTAssertEqual(h.calls(["app", "stop"]).last?.args, ["app", "stop", "sample-notes", "--instance", "2"])
        XCTAssertNil(h.store.appErrors["sample-notes--2"])
        await h.store.launchNewInstance("sample-notes")
        XCTAssertEqual(h.calls(["app", "run"]).last?.args, ["app", "run", "sample-notes", "--new"])
        XCTAssertNotNil(h.store.app(named: "sample-notes--3"), "the list is read again after --new")
        XCTAssertTrue(h.calls(["app", "run"]).allSatisfy { !$0.args.contains("--json") })
    }

    func testRefusedStartShowsPaddocksLine() async throws {
        let h = try await Harness("library")
        h.runner.reset(prefix: "app run")
        h.runner.on(prefix: "app run", output: CommandOutput(status: 1, stdout: Data(),
            stderr: "starting\nTwo macOS VMs run: sample-mail (pid 811), notes (pid 902). Stop one of them.\n"))
        await h.store.startApp("sample-notes--2")
        let error = try XCTUnwrap(h.store.appErrors["sample-notes--2"])
        XCTAssertEqual(error.title?.english, "Sample Notes 2 did not start")
        XCTAssertEqual(error.body?.english, "Two macOS VMs run: sample-mail (pid 811), notes (pid 902). Stop one of them.")
    }

    func testLateExitIsNotAStartError() {
        XCTAssertTrue(LibraryActions.isStartRefusal(status: 1, elapsed: 2))
        XCTAssertFalse(LibraryActions.isStartRefusal(status: 1, elapsed: LibraryActions.startWindow + 1),
                       "a session that ran and ended is no refused start")
        XCTAssertFalse(LibraryActions.isStartRefusal(status: 0, elapsed: 1))
    }

    func testSuspendNeedsTheCapability() async throws {
        let without = try await Harness("library")
        XCTAssertFalse(without.store.capabilities.suspend)
        await without.store.suspend("sample-notes")
        XCTAssertTrue(without.calls(["app", "suspend"]).isEmpty)

        let with = try await Harness("library-suspend")
        XCTAssertTrue(with.store.capabilities.suspend)
        await with.store.suspend("sample-notes")
        XCTAssertEqual(with.calls(["app", "suspend"]).last?.args, ["app", "suspend", "sample-notes", "--json"])
        XCTAssertNil(with.store.appErrors["sample-notes"])
    }

    func testCapabilitiesParse() throws {
        let plain = try String(contentsOf: Fixtures.base("app-help.out"), encoding: .utf8)
        XCTAssertFalse(Capabilities.parse(appHelp: plain).suspend)
        let more = try String(contentsOf: Fixtures.base("app-help-suspend.out"), encoding: .utf8)
        XCTAssertTrue(Capabilities.parse(appHelp: more).suspend)
        // A word in a description is no subcommand.
        XCTAssertFalse(Capabilities.parse(appHelp: "  stop    Stop or suspend a session\n").suspend)
    }

    // MARK: Add

    func testReviewChecklists() async throws {
        let h = try await Harness("drop")
        let ok = await h.store.review(path: "/Applications/Notebook.app")
        XCTAssertTrue(ok.canAdd)
        XCTAssertEqual(ok.checks.map(\.kind), [.bundle, .appStore, .golden, .space])
        XCTAssertEqual(ok.checks.map(\.status), [.pass, .pass, .pass, .pass])
        XCTAssertEqual(ok.checks[3].detail.english, "184.0 GB free, 2.7 GB needed")
        XCTAssertEqual(ok.name, "notebook")
        XCTAssertEqual(ok.displayName, "Notebook")
        XCTAssertEqual(ok.memoryOptions, [4, 6, 8])
        XCTAssertEqual(ok.defaultMemory, 6)

        let store = await h.store.review(path: "/Applications/Sample Tasks.app")
        XCTAssertFalse(store.canAdd)
        XCTAssertEqual(store.firstFailure?.kind, .appStore)
        XCTAssertTrue(store.firstFailure!.detail.english.hasPrefix("Sample Tasks comes from the App Store"))

        let existing = await h.store.review(path: "/Applications/Sample Notes.app")
        XCTAssertFalse(existing.canAdd)
        XCTAssertEqual(existing.existing, "sample-notes")

        let noGolden = try await Harness("drop-no-golden")
        let ng = await noGolden.store.review(path: "/Applications/Notebook.app")
        XCTAssertEqual(ng.firstFailure?.kind, .golden)
        XCTAssertEqual(ng.firstFailure?.action, .build)

        let low = try await Harness("drop-low-space")
        let ls = await low.store.review(path: "/Applications/Notebook.app")
        XCTAssertEqual(ls.firstFailure?.kind, .space)
        XCTAssertEqual(ls.firstFailure?.action, .manageStorage)
        XCTAssertEqual(ls.firstFailure?.detail.english, "Needs 2.7 GB on the volume of the macvm folder, 1.1 GB free.")

        // The older precheck fixtures make the same checklist.
        let legacy = try await Harness("precheck-app-store")
        let lr = await legacy.store.review(path: "/Applications/Sample Tasks.app")
        XCTAssertEqual(lr.firstFailure?.kind, .appStore)
        XCTAssertEqual(lr.firstFailure?.action, .openWebsite("https://tasks.example.com/sample-tasks/"))
    }

    func testReviewOfflineAndUnknownRefusal() async throws {
        let offline = LauncherStore(service: nil)
        let r = await offline.review(path: "/Applications/Notebook.app")
        XCTAssertEqual(r.failure, LibraryCopy.offline.english)
        XCTAssertFalse(r.canAdd)

        var report = try Fixtures.decode(AddReportJSON.self, "add-review-ok.json")
        report.refusals = [AddRefusalJSON(code: "new_reason", text: "A reason this launcher does not know.")]
        let review = AddReview.of(report: report, path: "/Applications/Notebook.app")
        XCTAssertFalse(review.canAdd, "an unknown refusal still blocks the add")
    }

    func testMemoryOptionsFollowMax() {
        XCTAssertEqual(AddReview.memoryOptions(AddMemoryJSON(mib: 4096, defaultMib: 4096, maxMib: 4096)), [4])
        XCTAssertEqual(AddReview.memoryOptions(AddMemoryJSON(mib: 6144, defaultMib: 6144, maxMib: 6144)), [4, 6])
        XCTAssertEqual(AddReview.memoryOptions(AddMemoryJSON(mib: nil, defaultMib: nil, maxMib: nil)), [4, 6, 8])
        XCTAssertEqual(AddReview.memoryOptions(AddMemoryJSON(mib: 2048, defaultMib: 2048, maxMib: 2048)), [4])
        XCTAssertNil(AddReview.memoryOptions(nil))
    }

    func testAddRunsAddThenInstall() async throws {
        let h = try await Harness("drop")
        let outcome = await h.store.add(path: "/Applications/Notebook.app", name: "notebook", memoryGB: 8)
        XCTAssertEqual(outcome, .added(key: "notebook"))
        XCTAssertEqual(h.calls(["app", "add", "/Applications/Notebook.app", "--name"]).last?.args,
                       ["app", "add", "/Applications/Notebook.app", "--name", "notebook", "--memory", "8G", "--json"])
        XCTAssertEqual(h.calls(["app", "install"]).last?.args, ["app", "install", "notebook"])
        XCTAssertEqual(h.store.selection, .app("notebook"))
        XCTAssertNotNil(h.store.app(named: "notebook"))
        XCTAssertEqual(h.store.toast?.text.english, "notebook added. It also appears in Launchpad as \"Notebook (paddock)\".")
    }

    func testAddRefusalKeepsTheLine() async throws {
        let h = try await Harness("drop")
        h.runner.reset(prefix: "app add /Applications/Notebook.app --name")
        h.runner.on(prefix: "app add /Applications/Notebook.app --name",
                    output: CommandOutput(status: 1, stdout: try Fixtures.data("add-review-low-space.json"), stderr: ""))
        let outcome = await h.store.add(path: "/Applications/Notebook.app", name: "notebook", memoryGB: 6)
        XCTAssertEqual(outcome, .refused("Needs 2.7 GB on the volume of the macvm folder, 1.1 GB free."))
        XCTAssertTrue(h.calls(["app", "install"]).isEmpty)
    }

    // MARK: Remove

    func testRemoveNamesTheTrash() async throws {
        let h = try await Harness("library")
        let text = await h.store.removeApp("sample-mail", deleteData: false)
        XCTAssertEqual(h.calls(["app", "remove"]).last?.args, ["app", "remove", "sample-mail", "--yes", "--json"])
        XCTAssertEqual(text?.english, "Sample Mail removed. 3.2 GB freed. Its data (2.1 GB) stays in the paddock trash for 7 days.")
        XCTAssertNil(h.store.app(named: "sample-mail"))

        let d = try await Harness("library")
        let deleted = await d.store.removeApp("sample-mail", deleteData: true)
        XCTAssertEqual(d.calls(["app", "remove"]).last?.args, ["app", "remove", "sample-mail", "--delete-data", "--yes", "--json"])
        XCTAssertEqual(deleted?.english, "Sample Mail removed. 7.3 GB freed.")
    }

    func testRemoveTakesEveryInstance() async throws {
        let h = try await Harness("library")
        await h.store.removeApp("sample-notes--2", deleteData: false)
        XCTAssertEqual(h.calls(["app", "remove"]).last?.args, ["app", "remove", "sample-notes", "--yes", "--json"])
        XCTAssertTrue(h.store.instances(ofApp: "sample-notes").isEmpty)
    }

    func testRemoveErrorJSON() async throws {
        let h = try await Harness("library")
        // The longest key wins: replace the answer for sample-mail itself.
        h.runner.reset(prefix: "app remove sample-mail --yes")
        h.runner.on(prefix: "app remove sample-mail --yes", output: CommandOutput(status: 1,
            stdout: Data(#"{"v":1,"error":"sample-mail runs. Stop it first: paddock app stop sample-mail"}"#.utf8), stderr: ""))
        let text = await h.store.removeApp("sample-mail", deleteData: false)
        XCTAssertNil(text)
        XCTAssertEqual(h.store.lastError?.body?.english, "sample-mail runs. Stop it first: paddock app stop sample-mail")
        XCTAssertNotNil(h.store.app(named: "sample-mail"))
    }

    func testRemoveTexts() throws {
        let apps = try LibraryDecode.apps(Fixtures.data("profiles-instances.json")).map(AppItem.init(json:))
        XCTAssertEqual(LibraryCopy.removeTitle(apps[0], instances: 2).english, "Remove Sample Notes and its 2 instances from paddock?")
        XCTAssertEqual(LibraryCopy.removeTitle(apps[2], instances: 1).english, "Remove Sample Mail from paddock?")
        XCTAssertTrue(LibraryCopy.removeBody(apps[2]).english.contains("Its data (2.1 GB) goes to the paddock trash for 7 days."))
    }

    // MARK: Build

    func testPauseAnswers() async throws {
        let h = try await Harness("building-v1")
        guard case .building = h.store.golden else { return XCTFail("\(h.store.golden)") }
        await h.store.pauseBuild()
        XCTAssertEqual(h.calls(["macvm", "setup", "--pause-download"]).last?.args, ["macvm", "setup", "--pause-download", "--json"])
        XCTAssertEqual(h.store.buildNote, LibraryCopy.pastDownload)
        if case .building(let p) = h.store.golden { XCTAssertFalse(p.paused) }

        h.runner.reset(prefix: "macvm setup --pause-download")
        h.runner.on(["macvm", "setup", "--pause-download"], json: try String(contentsOf: Fixtures.base("pause-requested.json"), encoding: .utf8))
        await h.store.pauseBuild()
        XCTAssertNil(h.store.buildNote)
        if case .building(let p) = h.store.golden { XCTAssertTrue(p.paused) }
    }

    func testBuildArguments() async throws {
        XCTAssertEqual(LibraryArgs.setup(ipsw: nil, resume: false), ["macvm", "setup", "--json", "--yes"])
        XCTAssertEqual(LibraryArgs.setup(ipsw: "/x.ipsw", resume: true), ["macvm", "setup", "--json", "--yes", "--ipsw", "/x.ipsw", "--resume"])
        XCTAssertEqual(LibraryArgs.macvmRemove, ["macvm", "remove", "--yes", "--json"])
        XCTAssertEqual(LibraryArgs.update, ["macvm", "update", "--yes"])
        let h = try await Harness("build-stopped-v1")
        guard case .failed(let failure) = h.store.golden else { return XCTFail("\(h.store.golden)") }
        XCTAssertEqual(failure.primary, .tryAgain)
        h.runner.feed(LibraryArgs.setup(ipsw: nil, resume: true)).send(
            #"{"v":1,"t":"2026-11-06T11:00:00Z","kind":"setup","step":"install","paused":false,"failed":"Needs 40.0 GiB on the volume, 12.1 GiB free.","stopped":false}"#)
        h.store.build(resume: true)
        await waitUntil { if case .failed(let f) = h.store.golden { return f.code == "no_space" } else { return false } }
        XCTAssertEqual(h.calls(["macvm", "setup", "--json"]).last?.args, ["macvm", "setup", "--json", "--yes", "--resume"])
        if case .failed(let f) = h.store.golden { XCTAssertEqual(f.primary, .manageStorage) }
    }

    // MARK: Storage

    func testStorageRows() throws {
        let du = try Fixtures.decode(DuReportJSON.self, "du.json")
        let apps = try LibraryDecode.apps(Fixtures.data("profiles-instances.json")).map(AppItem.init(json:))
        let report = try XCTUnwrap(StorageReport.of(du, apps: apps, freeBytes: 184_000_000_000))
        XCTAssertEqual(report.usageLine.english, "paddock uses 61.5 GB. 184.0 GB free on this disk.")
        XCTAssertEqual(report.rows.map(\.id), ["golden", "restoreImages", "instance:sample-notes", "instance:sample-mail",
                                               "instance:sample-notes--2", "trash", "shims", "agents"])
        let sampleMail = report.rows.first { $0.id == "instance:sample-mail" }!
        XCTAssertEqual(sampleMail.title.english, "Sample Mail")
        XCTAssertEqual(sampleMail.state?.english, "Suspended")
        XCTAssertTrue(sampleMail.inUse)
        XCTAssertEqual(sampleMail.detail?.english, "Saved state 4.1 GB · Backups 0.0 GB")
        XCTAssertEqual(report.rows.first { $0.id == "instance:sample-notes--2" }?.title.english, "Sample Notes 2")
        XCTAssertNil(StorageReport.of(try Fixtures.decode(DuReportJSON.self, "du-none.json"), apps: [], freeBytes: nil))
    }

    func testCleanupPreviewGroups() throws {
        let gc = try Fixtures.decode(GcReportJSON.self, "gc-dry.json")
        let plain = CleanupPreview.of(gc, includeLogins: false)
        XCTAssertEqual(plain.removedNow.count, 2)
        XCTAssertEqual(plain.kept.map(\.item.what), ["macvm profile sample-notes 2"])
        XCTAssertEqual(Format.gb(plain.bytesNow), "16.1 GB")
        XCTAssertEqual(plain.removeTitle.english, "Remove 16.1 GB")
        XCTAssertFalse(plain.hasOtherData)
        let logins = CleanupPreview.of(gc, includeLogins: true)
        XCTAssertEqual(logins.removedNow.count, 3)
        XCTAssertTrue(logins.kept.isEmpty)
        XCTAssertEqual(logins.flags, ["--homes", "--include-app-logins"])
        // An item that needs a flag the launcher never gives stays.
        let worktree = GcItemJSON(what: "worktree x", tier: "consent", bytes: 1, needs: ["--worktrees"], action: GcActionJSON(kind: "worktree"))
        let other = GcItemJSON(what: "old image", tier: "auto", bytes: 1, action: GcActionJSON(kind: "image"))
        let mixed = CleanupPreview.of(GcReportJSON(apiVersion: nil, dryRun: true, items: [worktree, other], removed: nil, notes: nil),
                                      includeLogins: true)
        XCTAssertEqual(mixed.kept.map(\.item.what), ["worktree x"])
        XCTAssertTrue(mixed.hasOtherData)
    }

    func testCleanupResult() throws {
        let done = CleanupResult.of(try Fixtures.decode(GcReportJSON.self, "gc-yes.json"))
        XCTAssertEqual(done.line.english, "16.1 GB freed.")
        var gc = try Fixtures.decode(GcReportJSON.self, "gc-yes.json")
        gc.removed?[1].error = "Operation not permitted"
        let partial = CleanupResult.of(gc)
        XCTAssertEqual(partial.line.english, "2.1 GB freed. 1 items were not removed.")
        XCTAssertEqual(partial.failures.first?.line, "Operation not permitted")
    }

    func testStorageModelFlow() async throws {
        let h = try await Harness("library")
        let model = StorageModel()
        await model.load(h.store)
        guard case .loaded(let report) = model.phase else { return XCTFail("\(model.phase)") }
        XCTAssertEqual(report.rows.count, 8)
        await model.preview(h.store)
        XCTAssertEqual(h.calls(["gc"]).last?.args, ["gc", "--json"])
        guard case .preview(let preview) = model.cleanup else { return XCTFail("\(model.cleanup)") }
        XCTAssertFalse(preview.includeLogins)
        model.includeLogins = true
        await model.preview(h.store)
        XCTAssertEqual(h.calls(["gc"]).last?.args, ["gc", "--homes", "--include-app-logins", "--json"])
        await model.apply(h.store)
        XCTAssertEqual(h.calls(["gc", "--yes"]).last?.args, ["gc", "--yes", "--homes", "--include-app-logins", "--json"])
        XCTAssertEqual(model.cleanup, .done(CleanupResult(freedBytes: 16_100_000_000, failures: [])))
        XCTAssertEqual(h.store.toast?.text.english, "16.1 GB freed.")
        model.closeCleanup()
        XCTAssertEqual(model.cleanup, .closed)
        XCTAssertFalse(model.includeLogins)

        let empty = try await Harness("storage-empty")
        let e = StorageModel()
        await e.load(empty.store)
        XCTAssertEqual(e.phase, .empty)

        let offline = StorageModel()
        await offline.load(LauncherStore(service: nil))
        XCTAssertEqual(offline.phase, .offline)

        let failing = try await Harness("ready")
        let f = StorageModel()
        await f.load(failing.store)
        if case .failed = f.phase {} else { XCTFail("\(f.phase)") }
    }

    // MARK: Drops

    func testDropIntake() {
        let intake = DropIntake.sort(["/Applications/Notebook.app/", "/Applications/Sample Tasks.app", "/Users/demo/notes.txt",
                                      "/Applications/Notebook.app", "/Applications/Odd.APP"])
        XCTAssertEqual(intake.apps, ["/Applications/Notebook.app", "/Applications/Sample Tasks.app", "/Applications/Odd.APP"])
        XCTAssertEqual(intake.rejected, ["/Users/demo/notes.txt"])
        let many = DropIntake.sort((1...12).map { "/Applications/A\($0).app" })
        XCTAssertEqual(many.apps.count, DropIntake.maxApps)
        XCTAssertEqual(many.rejected.count, 2)
    }

    func testAddQueue() {
        var queue = AddQueue()
        XCTAssertNil(queue.position)
        queue.enqueue(["/a.app"])
        XCTAssertNil(queue.position, "one app needs no counter")
        queue.enqueue(["/b.app", "/a.app", "/c.app"])
        XCTAssertEqual(queue.current, "/a.app")
        XCTAssertEqual(queue.position?.english, "App 1 of 3")
        queue.advance()
        XCTAssertEqual(queue.current, "/b.app")
        XCTAssertEqual(queue.position?.english, "App 2 of 3")
        queue.advance()
        queue.advance()
        XCTAssertTrue(queue.isEmpty)
        XCTAssertEqual(queue.total, 0)
        queue.enqueue(["/d.app", "/e.app"])
        XCTAssertEqual(queue.position?.english, "App 1 of 2")
        queue.clear()
        XCTAssertNil(queue.current)
    }

    // MARK: The launcher's own install

    func testSelfInstallPlaces() {
        let home = "/Users/demo"
        XCTAssertEqual(SelfInstall.place(bundlePath: "/Applications/paddock for Mac.app", home: home, readOnlyVolume: false), .applications)
        XCTAssertEqual(SelfInstall.place(bundlePath: "/Users/demo/Applications/paddock for Mac.app", home: home, readOnlyVolume: false),
                       .userApplications)
        XCTAssertEqual(SelfInstall.place(bundlePath: "/Users/demo/Downloads/paddock for Mac.app", home: home, readOnlyVolume: false),
                       .elsewhere)
        XCTAssertEqual(SelfInstall.place(bundlePath: "/Volumes/paddock/paddock for Mac.app", home: home, readOnlyVolume: true),
                       .readOnlyVolume)
        XCTAssertEqual(SelfInstall.place(bundlePath: "/private/var/folders/x/T/AppTranslocation/1234/d/paddock for Mac.app",
                                         home: home, readOnlyVolume: true), .translocated)
        XCTAssertEqual(SelfInstall.place(bundlePath: "/Users/demo/Library/Developer/Xcode/DerivedData/P/Build/Products/Debug/paddock for Mac.app",
                                         home: home, readOnlyVolume: false), .buildFolder)
        // A folder whose name starts like Applications is not it.
        XCTAssertEqual(SelfInstall.place(bundlePath: "/Users/demo/ApplicationsOld/paddock for Mac.app", home: home, readOnlyVolume: false),
                       .elsewhere)
    }

    func testSelfInstallRules() {
        XCTAssertTrue(SelfInstall.shouldOffer(.elsewhere, declined: false))
        XCTAssertTrue(SelfInstall.shouldOffer(.translocated, declined: false))
        XCTAssertTrue(SelfInstall.shouldOffer(.readOnlyVolume, declined: false))
        XCTAssertFalse(SelfInstall.shouldOffer(.elsewhere, declined: true))
        XCTAssertFalse(SelfInstall.shouldOffer(.applications, declined: false))
        XCTAssertFalse(SelfInstall.shouldOffer(.userApplications, declined: false))
        XCTAssertFalse(SelfInstall.shouldOffer(.buildFolder, declined: false))
        XCTAssertTrue(SelfInstall.removesOriginal(.elsewhere))
        XCTAssertFalse(SelfInstall.removesOriginal(.translocated))
        XCTAssertFalse(SelfInstall.removesOriginal(.readOnlyVolume))
        XCTAssertEqual(SelfInstall.target(home: "/Users/demo/"), "/Users/demo/Applications/paddock for Mac.app")
        XCTAssertEqual(SelfInstall.decision(existing: nil, mine: "0.1.0"), .copy)
        XCTAssertEqual(SelfInstall.decision(existing: "0.0.9", mine: "0.1.0"), .replace)
        XCTAssertEqual(SelfInstall.decision(existing: "0.1.0", mine: "0.1.0"), .openExisting)
        XCTAssertEqual(SelfInstall.decision(existing: "0.10.0", mine: "0.9.0"), .openExisting)
        XCTAssertEqual(SelfInstall.compare("1.2", "1.2.0"), 0)
    }

    func testVersionsLine() {
        XCTAssertEqual(LibraryCopy.versions(launcher: "0.1.0", paddock: "0.2.2").english, "paddock for Mac 0.1.0 · paddock 0.2.2")
        XCTAssertEqual(LibraryCopy.versions(launcher: "0.1.0", paddock: nil).english, "paddock for Mac 0.1.0")
    }
}
