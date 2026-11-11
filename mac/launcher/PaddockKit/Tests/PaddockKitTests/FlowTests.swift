import XCTest
@testable import PaddockKit

/// Add app, questions, quit, remove, uninstall and the menu bar model.
@MainActor
final class FlowTests: XCTestCase {
    // MARK: Add app

    func testPrecheckVerdicts() async throws {
        let ok = try await Harness("ready")
        let (pre, verdict) = await ok.store.precheck(path: "/Applications/Notebook.app")
        XCTAssertEqual(verdict, .ok)
        XCTAssertEqual(pre?.memoryOptionsGb, [4, 6, 8])
        XCTAssertTrue(verdict.canAdd)

        let store = try await Harness("precheck-app-store")
        let (_, appStore) = await store.store.precheck(path: "/Applications/Sample Tasks.app")
        XCTAssertEqual(appStore, .appStore(website: "https://tasks.example.com/sample-tasks/"))
        XCTAssertFalse(appStore.canAdd)
        XCTAssertEqual(appStore.message?.english,
                       "This app came from the App Store and needs an Apple ID, which the sandbox does not have. Install the version from the maker's website instead.")

        let noGolden = try await Harness("precheck-no-golden")
        let (_, ng) = await noGolden.store.precheck(path: "/Applications/Notebook.app")
        XCTAssertEqual(ng, .noGolden)
        XCTAssertEqual(ng.message?.english, "Build the base image first.")

        let low = try await Harness("precheck-low-space")
        let (_, ls) = await low.store.precheck(path: "/Applications/Notebook.app")
        XCTAssertEqual(ls.message?.english, "Needs 2.7 GB, 1.1 GB free.")
        XCTAssertEqual(low.calls(["app", "add", "/Applications/Notebook.app", "--dry-run", "--json"]).count, 1)
    }

    func testAddAppAppendsRowAndNamesLaunchpad() async throws {
        let h = try await Harness("added")
        XCTAssertEqual(h.store.apps.count, 2)
        let added = await h.store.addApp(path: "/Applications/Notebook.app", name: "Notebook", memoryGB: 6)
        XCTAssertTrue(added)
        XCTAssertEqual(h.store.apps.map(\.name), ["Sample Notes", "Sample Mail", "Notebook"])
        XCTAssertEqual(h.store.selection, .app("Notebook"))
        XCTAssertEqual(h.store.toast?.text.english,
                       "Notebook added. It also appears in Launchpad as \"Notebook (paddock)\".")
        XCTAssertEqual(h.calls(["app", "add", "/Applications/Notebook.app", "--name"]).first?.args,
                       ["app", "add", "/Applications/Notebook.app", "--name", "Notebook", "--memory", "6G", "--json"])
        XCTAssertEqual(h.calls(["app", "install", "Notebook"]).count, 1)
    }

    // MARK: Questions

    func testQuestionOpenAnswerAndPause() async throws {
        let h = try await Harness("questions")
        XCTAssertEqual(h.store.questions.count, 2)
        XCTAssertEqual(MenuModel.of(h.store, keepRunning: true).questionsItem?.english, "Open questions… (2)")
        h.events.send(#"{"v":1,"kind":"question","op":"open","id":"q3","app":"Sample Notes","host":"files.files.example.com","port":443,"process":"Sample Notes.app","opened":"2026-11-06T10:00:00Z"}"#)
        await waitUntil { h.store.questions.count == 3 }
        XCTAssertEqual(MenuModel.of(h.store, keepRunning: true).questionsItem?.english, "Open questions… (3)")
        XCTAssertEqual(MenuModel.of(h.store, keepRunning: true).iconState, .question)

        // Answering from the list sends the answer for that id.
        let q1 = h.store.questions.first { $0.id == "q1" }!
        await h.store.answer(q1, .allowOnce)
        XCTAssertEqual(h.calls(["approvals", "answer"]).last?.args, ["approvals", "answer", "q1", "--allow-once"])
        XCTAssertFalse(h.store.questions.contains { $0.id == "q1" })

        // An answer from paddock's panel removes the row.
        h.events.send(#"{"v":1,"kind":"question","op":"answered","id":"q3","answer":"allow-always","source":"panel"}"#)
        await waitUntil { !h.store.questions.contains { $0.id == "q3" } }

        // Asking paused: the banner state.
        h.events.send(#"{"v":1,"kind":"question","op":"paused","app":"Sample Notes","count":23}"#)
        await waitUntil { h.store.paused != nil }
        XCTAssertEqual(QuestionsCopy.pausedBanner(h.store.paused!).english,
                       "Sample Notes asked 23 times in a minute. paddock paused the questions.")
        await h.store.pausedDenyAll()
        XCTAssertEqual(h.calls(["approvals", "paused"]).last?.args, ["approvals", "paused", "Sample Notes", "--deny-all"])
        XCTAssertNil(h.store.paused)
    }

    func testEveryAnswerFlag() {
        XCTAssertEqual(Answer.allCases.map(\.flag),
                       ["--deny", "--allow-once", "--allow-session", "--allow-always", "--deny-always"])
    }

    func testNotificationAfterThreshold() {
        let opened = ISO.date("2026-11-06T10:00:00Z")
        let q = QuestionJSON(id: "a", app: "Sample Notes", host: "h", port: 443, opened: opened)
        XCTAssertTrue(QuestionNotifier.due([q], now: opened.addingTimeInterval(29), threshold: 30, notified: []).isEmpty)
        XCTAssertEqual(QuestionNotifier.due([q], now: opened.addingTimeInterval(30), threshold: 30, notified: []).count, 1)
        XCTAssertTrue(QuestionNotifier.due([q], now: opened.addingTimeInterval(300), threshold: 30, notified: ["a"]).isEmpty)
        XCTAssertTrue(QuestionNotifier.due([q], now: opened.addingTimeInterval(300), threshold: nil, notified: []).isEmpty)
        XCTAssertEqual(QuestionNotifier.title(q).english, "Sample Notes is waiting for an answer")
    }

    // MARK: Quit

    func testQuitKeepsAppsRunningBySetting() async throws {
        let h = try await Harness("ready")
        XCTAssertEqual(QuitPlanner.plan(keepRunning: true, apps: h.store.apps, building: false), [])
        await h.store.performQuit(keepRunning: true)
        XCTAssertTrue(h.calls(["app", "stop"]).isEmpty)
        XCTAssertEqual(h.store.quitNote(keepRunning: true)?.english, "1 app keeps running")
    }

    func testQuitStopsAppsWhenSettingOff() async throws {
        let h = try await Harness("ready")
        XCTAssertEqual(QuitPlanner.plan(keepRunning: false, apps: h.store.apps, building: false), [.stop("Sample Notes")])
        let quit = Task { await h.store.performQuit(keepRunning: false, timeout: 60) }
        await waitUntil { h.calls(["app", "stop", "Sample Notes"]).count == 1 }
        h.events.send(#"{"v":1,"kind":"session","app":"Sample Notes","state":"saved","since":"2026-11-06T10:00:05Z"}"#)
        await waitUntil { if case .saved = h.store.app(named: "Sample Notes")!.state { return true } else { return false } }
        // The stream watcher sleeps too; wait for the quit loop's own sleep.
        await waitUntil { h.clock.sleeperCount >= 2 }
        h.clock.advance(1)
        await quit.value
        XCTAssertEqual(h.calls(["app", "stop"]).count, 1)
    }

    func testQuitDuringBuildPauses() async throws {
        let h = try await Harness("fresh")
        h.store.build()
        await settle()
        XCTAssertTrue(h.store.isBuilding)
        XCTAssertEqual(QuitPlanner.plan(keepRunning: true, apps: [], building: true), [.pauseBuild])
        await h.store.performQuit(keepRunning: true)
        XCTAssertEqual(h.calls(PaddockArgs.setupPause).count, 1)
    }

    // MARK: Remove app

    func testRemoveSheetSizesAndCalls() async throws {
        let h = try await Harness("ready")
        let sampleMail = h.store.app(named: "Sample Mail")!
        let sheet = RemovePresentation.of(sampleMail)
        XCTAssertEqual(sheet.title.english, "Remove Sample Mail from paddock?")
        XCTAssertEqual(sheet.body.english,
                       "This removes the sandbox copy of the app (1.2 GB), its saved state (2.0 GB) and the launcher in your Applications folder. Its data (3.4 GB) goes to the Trash, where you can get it back. The original app in /Applications stays.")
        XCTAssertFalse(sheet.blockedByRunning)
        XCTAssertTrue(RemovePresentation.of(h.store.app(named: "Sample Notes")!).blockedByRunning)
        XCTAssertEqual(RemoveCopy.stopFirst.english, "Stop the app first.")

        await h.store.removeApp("Sample Mail", deleteData: false)
        XCTAssertEqual(h.calls(["app", "remove"]).last?.args, ["app", "remove", "Sample Mail", "--yes", "--json"])
        XCTAssertEqual(h.store.toast?.text.english, "Sample Mail removed. 6.6 GB freed.")
        XCTAssertNil(h.store.app(named: "Sample Mail"))

        await h.store.removeApp("Sample Notes", deleteData: true)
        XCTAssertEqual(h.calls(["app", "remove"]).last?.args, ["app", "remove", "Sample Notes", "--delete-data", "--yes", "--json"])
    }

    // MARK: Uninstall

    func testUninstallOrderSizesAndNoBrew() async throws {
        let h = try await Harness("stopped")
        let sizes = UninstallPlan.sizes(apps: h.store.apps, golden: h.store.golden)
        XCTAssertEqual(Format.gb(sizes.apps), "6.4 GB")
        XCTAssertEqual(Format.gb(sizes.golden), "25.3 GB")
        XCTAssertEqual(Format.gb(sizes.data), "6.8 GB")
        let plan = UninstallPlan()
        XCTAssertEqual(plan.commands(apps: ["Sample Notes", "Sample Mail"]),
                       [["app", "remove", "Sample Notes", "--yes", "--json"], ["app", "remove", "Sample Mail", "--yes", "--json"], ["macvm", "remove", "--yes", "--json"]])
        let freed = await h.store.uninstall(plan)
        XCTAssertEqual(Format.gb(freed), "38.5 GB")
        let order = h.runner.calls.map(\.args).filter { $0.contains("remove") }
        XCTAssertEqual(order, [["app", "remove", "Sample Notes", "--yes", "--json"], ["app", "remove", "Sample Mail", "--yes", "--json"], ["macvm", "remove", "--yes", "--json"]])
        XCTAssertFalse(h.runner.calls.contains { $0.line.contains("brew") })
        XCTAssertEqual(UninstallPlan(removeData: true).commands(apps: ["A"]).first, ["app", "remove", "A", "--delete-data", "--yes", "--json"])
        XCTAssertEqual(UninstallCopy.done(freed).english, "Done. 38.5 GB freed. You can now drag paddock for Mac to the Trash.")
    }

    // MARK: Menu bar

    func testMenuListsAtMostEightApps() async throws {
        let h = try await Harness("many-apps")
        let menu = MenuModel.of(h.store, keepRunning: true)
        XCTAssertEqual(menu.apps.count, 8)
        XCTAssertTrue(menu.showsMore)
        XCTAssertNil(menu.quitNote)
        XCTAssertEqual(menu.header.english, "Base image ready")
        XCTAssertEqual(menu.iconState, .idle)
    }

    func testMenuHeaderAndQuitNote() async throws {
        let h = try await Harness("ready")
        var menu = MenuModel.of(h.store, keepRunning: true)
        XCTAssertFalse(menu.showsMore)
        XCTAssertEqual(menu.quitNote?.english, "1 app keeps running")
        XCTAssertEqual(menu.iconState, .running)
        h.events.send(#"{"v":1,"kind":"session","app":"Sample Mail","state":"running","windows":1}"#)
        await waitUntil { h.store.activeApps.count == 2 }
        menu = MenuModel.of(h.store, keepRunning: true)
        XCTAssertEqual(menu.quitNote?.english, "2 apps keep running")
        XCTAssertEqual(GoldenState.none.menuHeader.english, "No base image")
        var progress = BuildProgress()
        progress.apply(SetupJSON(step: "download", bytes: 48, total: 100))
        XCTAssertEqual(GoldenState.building(progress).menuHeader.english, "Building base image · 48 %")
    }

    // MARK: Stream loss and polling

    func testStreamLossShowsPausedAndRecovers() async throws {
        let h = try await Harness("ready")
        XCTAssertTrue(h.store.streamHealthy)
        await waitUntil { h.events.isOpen }
        h.events.kill()
        await waitUntil { !h.store.streamHealthy }
        XCTAssertEqual(StreamCopy.pausedLine.english, "Live updates paused · Refresh")
        // The stream comes back: the store reopens it after 5 s and counts it
        // healthy after it stays open for 2 s.
        await waitUntil { h.clock.sleeperCount > 0 }
        h.clock.advance(5)
        await waitUntil { h.events.opened >= 2 }
        await waitUntil { h.clock.sleeperCount > 0 }
        h.clock.advance(2)
        await waitUntil { h.store.streamHealthy }

        // Refresh recovers at once.
        h.events.kill()
        await waitUntil { !h.store.streamHealthy }
        await h.store.refresh()
        h.events.send(#"{"v":1,"kind":"disk","free_bytes":1}"#)
        await waitUntil { h.store.streamHealthy }
    }

    func testNoFasterThanOneStatusCallPerFiveSeconds() async throws {
        let h = try await Harness("ready")
        let detail = h.store.detail(for: "Sample Notes")
        for _ in 0..<10 {
            await detail.load()
            await h.store.refreshAll()
            h.clock.advance(1)
        }
        // Ten seconds of fake time: at most one call per 5 s per screen.
        let status = h.calls(["app", "status", "Sample Notes"])
        XCTAssertLessThanOrEqual(status.count, 2)
        XCTAssertLessThanOrEqual(h.calls(["macvm", "status"]).count, 2)
        for pair in zip(status, status.dropFirst()) {
            XCTAssertGreaterThanOrEqual(pair.1.time.timeIntervalSince(pair.0.time), 5)
        }
        // Grant events never cause more than the gate allows.
        for _ in 0..<5 {
            h.events.send(#"{"v":1,"kind":"grant","app":"Sample Notes","grant":"rule","op":"add","detail":"Allow always x.com","source":"question_panel"}"#)
        }
        await settle()
        XCTAssertLessThanOrEqual(h.calls(["app", "status", "Sample Notes"]).count, 3)
    }
}
