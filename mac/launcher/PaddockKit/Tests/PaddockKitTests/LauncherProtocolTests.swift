import Foundation
import XCTest
@testable import PaddockKit

/// `paddock --launcher-protocol`: the check of the answer, the store gate
/// and the limits of the bounded run.
final class LauncherProtocolTests: XCTestCase {
    func output(_ text: String, status: Int32 = 0, stderr: String = "") -> CommandOutput {
        CommandOutput(status: status, stdout: Data(text.utf8), stderr: stderr)
    }

    func protocolFile(_ name: String) throws -> Data {
        try Data(contentsOf: Fixtures.core("launcher-protocol/" + name))
    }

    // MARK: The answer

    // The answer of this core on a Mac, vendored from paddock core.
    func testTheAnswerOfThisCorePasses() throws {
        let answer = CommandOutput(status: 0, stdout: try protocolFile("mac-v1.json"), stderr: "")
        XCTAssertNil(LauncherProtocol.check(answer))
    }

    // A build without app mode lists no features.
    func testABuildWithoutAppModeMissesEveryFeature() throws {
        let answer = CommandOutput(status: 0, stdout: try protocolFile("other-v1.json"), stderr: "")
        XCTAssertEqual(LauncherProtocol.check(answer), .missingFeatures(LauncherProtocol.required.sorted()))
    }

    // paddock 0.2.2 and older: clap refuses the option with status 2.
    func testAPaddockWithoutTheQueryIsRefused() throws {
        let err = String(decoding: try protocolFile("unknown-option.err"), as: UTF8.self)
        XCTAssertTrue(err.contains("unexpected argument '--launcher-protocol'"))
        XCTAssertEqual(LauncherProtocol.check(output("", status: 2, stderr: err)), .noAnswer(status: 2))
        // A good answer with a failed exit is still no answer.
        let good = String(decoding: try protocolFile("mac-v1.json"), as: UTF8.self)
        XCTAssertEqual(LauncherProtocol.check(output(good, status: 1)), .noAnswer(status: 1))
    }

    func testTheHostileFixtures() throws {
        let cases: [(String, LauncherProtocol.Failure)] = [
            ("protocol-bad-json.json", .badAnswer),
            ("protocol-duplicate-key.json", .badAnswer),
            ("protocol-missing-feature.json", .missingFeatures(["events-v1"])),
            ("protocol-other.json", .otherProtocol("paddock.mac-launcher/v2")),
            ("protocol-oversized.json", .tooLarge),
        ]
        for (name, failure) in cases {
            let answer = CommandOutput(status: 0, stdout: try Fixtures.data(name), stderr: "")
            XCTAssertEqual(LauncherProtocol.check(answer), failure, name)
        }
    }

    func testEveryCriticalFieldIsChecked() {
        let features = #"["app-list-v1","approvals-v1","events-v1","macvm-status-v1","setup-progress-v1"]"#
        let bad = [
            "",
            "\n",
            "null",
            "[]",
            #"{}"#,
            #"{"features":\#(features)}"#,
            #"{"protocol":null,"features":\#(features)}"#,
            #"{"protocol":1,"features":\#(features)}"#,
            #"{"protocol":"paddock.mac-launcher/v1"}"#,
            #"{"protocol":"paddock.mac-launcher/v1","features":null}"#,
            #"{"protocol":"paddock.mac-launcher/v1","features":"events-v1"}"#,
            #"{"protocol":"paddock.mac-launcher/v1","features":[1]}"#,
            #"{"protocol":"paddock.mac-launcher/v1","features":["events-v1","events-v1"]}"#,
            #"{"protocol":"paddock.mac-launcher/v1","features":\#(features),"features":[]}"#,
            #"{"protocol":"paddock.mac-launcher/v1","features":\#(features)}{"protocol":"x"}"#,
            "{\"protocol\":\"paddock.mac-launcher/v1\",\"features\":\(features)}\n{}\n",
            #"{"protocol":"paddock.mac-launcher/v1","features":\#(features)} trailing"#,
            #"{"protocol":"paddock.mac-launcher/v1","features":\#(features),}"#,
            #"{'protocol':'paddock.mac-launcher/v1'}"#,
        ]
        for text in bad {
            XCTAssertEqual(LauncherProtocol.check(output(text)), .badAnswer, text)
        }
        var invalidUTF8 = Data(#"{"protocol":"paddock.mac-launcher/v1","features":["#.utf8)
        invalidUTF8.append(contentsOf: [0x22, 0xFF, 0xFE, 0x22, 0x5D, 0x7D])
        XCTAssertEqual(LauncherProtocol.check(CommandOutput(status: 0, stdout: invalidUTF8, stderr: "")), .badAnswer)
        // A higher version alone is not v1.
        XCTAssertEqual(LauncherProtocol.check(output(#"{"protocol":"paddock.mac-launcher/v10","features":\#(features)}"#)),
                       .otherProtocol("paddock.mac-launcher/v10"))
    }

    func testUnknownFeaturesAndFieldsAndWhitespaceAreAllowed() {
        let text = """
          { "protocol" : "paddock.mac-launcher/v1", "build": {"commit": "1a2b3c4", "n": [1, -2.5e3, true, null]},
            "features": ["setup-progress-v1", "future-v9", "macvm-status-v1", "events-v1", "approvals-v1", "app-list-v1"] }

        """
        XCTAssertNil(LauncherProtocol.check(output(text)))
    }

    func testTheRequiredFeaturesAreTheOnesOfThisCore() throws {
        guard case .object(let fields) = try StrictJSON.parse(try protocolFile("mac-v1.json")),
              case .array(let list)? = fields["features"] else { return XCTFail("not an object") }
        let features = list.compactMap { item -> String? in
            if case .string(let name) = item { return name }
            return nil
        }
        XCTAssertEqual(features, LauncherProtocol.required)
        XCTAssertFalse(features.contains("app-list-v1-completeness"))
    }

    // The pin names the core version that the vendored fixtures come from.
    func testThePinIsInRangeOfTheVersionGate() throws {
        let pin = try String(contentsOf: Fixtures.root.deletingLastPathComponent().appendingPathComponent("core.pin"),
                             encoding: .utf8)
        let rows = pin.split(separator: "\n").filter { !$0.hasPrefix("#") }
        let version = rows.first { $0.hasPrefix("version=") }.map { String($0.dropFirst("version=".count)) }
        let commit = rows.first { $0.hasPrefix("commit=") }.map { String($0.dropFirst("commit=".count)) }
        XCTAssertEqual(VersionGate.current.check(versionOutput: "paddock \(version ?? "")"),
                       .ok(version: version ?? ""))
        XCTAssertEqual(commit?.count, 40)
        XCTAssertNotEqual(commit, String(repeating: "0", count: 40))
        XCTAssertTrue(commit?.allSatisfy(\.isHexDigit) ?? false)
    }

    // MARK: The strict reader

    func testStrictJSONReadsWhatItShould() throws {
        XCTAssertEqual(try StrictJSON.parse(Data(#"{"a":"x\"\\\/\né😀"}"#.utf8)),
                       .object(["a": .string("x\"\\/\né😀")]))
        XCTAssertEqual(try StrictJSON.parse(Data("[0,-1,2.50,3e+2,true,false,null]".utf8)),
                       .array([.number("0"), .number("-1"), .number("2.50"), .number("3e+2"),
                               .bool(true), .bool(false), .null]))
        XCTAssertEqual(try StrictJSON.parse(Data(" {} \n".utf8)), .object([:]))
    }

    func testStrictJSONRefusesWhatItShould() {
        let deep = String(repeating: "[", count: StrictJSON.maxDepth + 1) + String(repeating: "]", count: StrictJSON.maxDepth + 1)
        for text in ["01", "1.", "-", ".5", "1e", "+1", "tru", "\"\u{01}\"", #""\x""#, #""\ud83d""#, #""\ude00""#,
                     #"{"a":1,"a":2}"#, #"{"a":{"b":1,"b":1}}"#, "[1,]", "[1 2]", "{\"a\"}", deep, "\"open"] {
            XCTAssertThrowsError(try StrictJSON.parse(Data(text.utf8)), text)
        }
        let ok = String(repeating: "[", count: StrictJSON.maxDepth) + String(repeating: "]", count: StrictJSON.maxDepth)
        XCTAssertNoThrow(try StrictJSON.parse(Data(ok.utf8)))
    }

    // MARK: The copy

    func testTheCopyOfEachFailure() {
        let rows: [(LauncherProtocol.Failure, String, String?, LauncherError.Primary)] = [
            (.noAnswer(status: 2), "Update paddock", ShellCommands.brewUpgrade, .copyCommand),
            (.badAnswer, "Update paddock", ShellCommands.brewUpgrade, .copyCommand),
            (.missingFeatures(["events-v1"]), "Update paddock", ShellCommands.brewUpgrade, .copyCommand),
            (.tooLarge, "Update paddock", ShellCommands.brewUpgrade, .copyCommand),
            (.otherProtocol("paddock.mac-launcher/v2"), "Update paddock for Mac", ShellCommands.brewUpgradeLauncher,
             .copyCommand),
            (.timedOut, "paddock did not answer", nil, .checkAgain),
            (.notRun, "paddock did not answer", nil, .checkAgain),
        ]
        for (reason, title, command, primary) in rows {
            let error = LauncherError.incompatible(version: "0.2.2", reason: reason)
            XCTAssertEqual(error.title?.english, title, "\(reason)")
            XCTAssertEqual(error.command, command, "\(reason)")
            XCTAssertEqual(error.primary, primary, "\(reason)")
            XCTAssertEqual(error.placement, .alert)
            XCTAssertFalse(error.body?.english.isEmpty ?? true)
            let row = Prerequisites.rows(doctor: nil, binary: .incompatible(version: "0.2.2", reason: reason),
                                         binaryMissing: false).first { $0.kind == .paddock }!
            XCTAssertEqual(row.status, .fail)
            XCTAssertEqual(row.command, command)
            XCTAssertEqual(row.result, error.body)
            XCTAssertTrue(row.actions.contains(.checkAgain))
        }
        XCTAssertEqual(LauncherError.incompatible(version: "0.2.2", reason: .noAnswer(status: 2)).body?.english,
                       "paddock 0.2.2 does not tell paddock for Mac what it supports. Update paddock, then check again.")
        XCTAssertEqual(VersionGate.current.error(for: .incompatible(version: "0.2.2", reason: .timedOut)),
                       .incompatible(version: "0.2.2", reason: .timedOut))
    }
}

/// The store does not use a paddock that fails the check: no probe, no
/// snapshot, no stream, no action, not even after Refresh.
@MainActor
final class LauncherProtocolStoreTests: XCTestCase {
    /// Every call the store makes after a passed check.
    let afterCheck: [[String]] = [["app", "--help"], PaddockArgs.macvmStatus, PaddockArgs.appList,
                                  PaddockArgs.approvals, PaddockArgs.events, PaddockArgs.doctor]

    func assertUnusable(_ h: Harness, _ reason: LauncherProtocol.Failure, version: String = "0.2.2",
                        file: StaticString = #filePath, line: UInt = #line) async {
        XCTAssertEqual(h.store.binary, .found(path: "/fake/paddock", verdict: .incompatible(version: version, reason: reason)),
                       file: file, line: line)
        XCTAssertFalse(h.store.binary.isUsable, file: file, line: line)
        XCTAssertNil(h.store.service, file: file, line: line)
        XCTAssertTrue(h.store.needsOnboarding, file: file, line: line)
        await h.store.refresh()
        await h.store.refreshAll()
        h.store.startEvents()
        await h.store.reloadStatus()
        await h.store.reloadList()
        await settle()
        for prefix in afterCheck {
            XCTAssertTrue(h.calls(prefix).isEmpty, "\(prefix) ran", file: file, line: line)
        }
        XCTAssertFalse(h.events.isOpen, file: file, line: line)
        XCTAssertEqual(h.calls(PaddockArgs.version).count, 1, file: file, line: line)
        XCTAssertEqual(h.calls(PaddockArgs.launcherProtocol).count, 1, file: file, line: line)
        // Every call was a whole-word query, never with other arguments.
        XCTAssertEqual(h.calls(PaddockArgs.launcherProtocol).first?.args, ["--launcher-protocol"], file: file, line: line)
    }

    func testAGood022BuildIsUsable() async throws {
        let h = try await Harness("protocol-good")
        XCTAssertEqual(h.store.binary, .found(path: "/fake/paddock", verdict: .ok(version: "0.2.2")))
        XCTAssertNotNil(h.store.service)
        XCTAssertEqual(h.calls(PaddockArgs.version).count, 1)
        XCTAssertEqual(h.calls(PaddockArgs.launcherProtocol).count, 1)
        // The version comes first, the query second, the snapshots after.
        let order = h.runner.calls.map(\.args)
        XCTAssertEqual(Array(order.prefix(2)), [PaddockArgs.version, PaddockArgs.launcherProtocol])
        XCTAssertFalse(h.calls(PaddockArgs.appList).isEmpty)
        XCTAssertTrue(h.events.isOpen)
    }

    func testTheReleased022WithoutTheQueryIsRefused() async throws {
        await assertUnusable(try await Harness("protocol-none"), .noAnswer(status: 2))
    }

    func testABadAnswerIsRefused() async throws {
        await assertUnusable(try await Harness("protocol-bad-json"), .badAnswer)
        await assertUnusable(try await Harness("protocol-duplicate-key"), .badAnswer)
    }

    func testAMissingFeatureIsRefused() async throws {
        await assertUnusable(try await Harness("protocol-missing-feature"), .missingFeatures(["events-v1"]))
    }

    func testAnotherProtocolIsRefused() async throws {
        await assertUnusable(try await Harness("protocol-other"), .otherProtocol("paddock.mac-launcher/v2"))
    }

    func testAnOversizedAnswerIsRefusedAndKilled() async throws {
        let h = try await Harness("protocol-oversized")
        await assertUnusable(h, .tooLarge)
        XCTAssertEqual(h.runner.kills, 1)
    }

    // The query hangs: the store stays `checking` until the time limit,
    // then the child is killed and paddock is refused.
    func testAHangingQueryEndsAtTheLimit() async throws {
        let h = try await Harness("protocol-hang", start: false)
        let service = h.service
        let start = Task { await h.store.start(locate: { "/fake/paddock" }, makeService: { _ in service }) }
        await waitUntil { h.runner.hanging == 1 && h.clock.sleeperCount == 1 }
        XCTAssertEqual(h.store.binary, .checking)
        XCTAssertNil(h.store.service)
        h.clock.advance(LauncherProtocol.limits.seconds - 1)
        await settle()
        XCTAssertEqual(h.store.binary, .checking)
        h.clock.advance(1)
        await start.value
        XCTAssertEqual(h.runner.hanging, 0)
        XCTAssertEqual(h.runner.kills, 1)
        await assertUnusable(h, .timedOut)
    }

    func testAHangingVersionEndsAtTheLimit() async throws {
        let h = try await Harness("version-hang", start: false)
        let service = h.service
        let start = Task { await h.store.start(locate: { "/fake/paddock" }, makeService: { _ in service }) }
        await waitUntil { h.runner.hanging == 1 && h.clock.sleeperCount == 1 }
        h.clock.advance(LauncherProtocol.limits.seconds)
        await start.value
        XCTAssertEqual(h.store.binary, .found(path: "/fake/paddock", verdict: .incompatible(version: "", reason: .timedOut)))
        XCTAssertTrue(h.calls(PaddockArgs.launcherProtocol).isEmpty)
        XCTAssertNil(h.store.service)
    }

    // Check Again while the first check hangs: the first child is killed,
    // its late result is dropped, the second start wins.
    func testCheckAgainCancelsAHangingCheck() async throws {
        let hung = try await Harness("protocol-hang", start: false)
        let good = try await Harness("protocol-good", start: false)
        let store = hung.store
        let first = Task { await store.start(locate: { "/fake/paddock" }, makeService: { _ in hung.service }) }
        await waitUntil { hung.runner.hanging == 1 }
        await store.start(locate: { "/fake/paddock" }, makeService: { _ in good.service })
        await first.value
        XCTAssertEqual(hung.runner.hanging, 0)
        XCTAssertEqual(hung.runner.kills, 1)
        XCTAssertEqual(store.binary, .found(path: "/fake/paddock", verdict: .ok(version: "0.2.2")))
        XCTAssertTrue(store.service === good.service)
        // The first paddock got no read after its check.
        XCTAssertTrue(hung.calls(PaddockArgs.appList).isEmpty)
        XCTAssertTrue(hung.calls(PaddockArgs.events).isEmpty)
    }

    // Check Again on a usable paddock that became incompatible: the old
    // stream stops and no later read reaches the old service.
    func testCheckAgainStopsTheOldStreamAndFencesItsReads() async throws {
        let first = try await Harness("protocol-good")
        XCTAssertTrue(first.events.isOpen)
        let store = first.store
        let old = try await Harness("protocol-none", start: false)
        await store.start(locate: { "/fake/paddock" }, makeService: { _ in old.service })
        XCTAssertEqual(store.binary, .found(path: "/fake/paddock", verdict: .incompatible(version: "0.2.2", reason: .noAnswer(status: 2))))
        XCTAssertNil(store.service)
        // A line on the old stream no longer reaches the store.
        let approvals = first.calls(PaddockArgs.approvals).count
        let lists = first.calls(PaddockArgs.appList).count
        first.events.send(#"{"v":1,"kind":"stream","state":"started"}"#)
        await store.refresh()
        store.startEvents()
        await settle()
        XCTAssertEqual(first.calls(PaddockArgs.approvals).count, approvals)
        XCTAssertEqual(first.calls(PaddockArgs.appList).count, lists)
        XCTAssertEqual(first.calls(PaddockArgs.events).count, 1)
        XCTAssertTrue(old.calls(PaddockArgs.appList).isEmpty)
        XCTAssertTrue(old.calls(PaddockArgs.events).isEmpty)
        XCTAssertFalse(store.streamHealthy)
    }

    // The too-old and too-new copy stays: the query is not asked then.
    func testOldAndNewVersionsAreNotQueried() async throws {
        let old = try await Harness("old-version")
        XCTAssertEqual(old.store.binary, .found(path: "/fake/paddock", verdict: .tooOld(have: "0.2.1", need: "0.2.2")))
        XCTAssertTrue(old.calls(PaddockArgs.launcherProtocol).isEmpty)
        let new = try await Harness("new-version")
        XCTAssertEqual(new.store.binary, .found(path: "/fake/paddock", verdict: .tooNew(have: "1.0.0")))
        XCTAssertTrue(new.calls(PaddockArgs.launcherProtocol).isEmpty)
    }

    // A bounded replay run that is cancelled counts as a kill.
    func testACancelledHangIsKilled() async throws {
        let runner = ReplayRunner(clock: FakeClock())
        runner.hang(prefix: "--launcher-protocol")
        let task = Task { try await runner.run(PaddockArgs.launcherProtocol, limits: LauncherProtocol.limits) }
        await waitUntil { runner.hanging == 1 }
        task.cancel()
        do {
            _ = try await task.value
            XCTFail("expected a cancel")
        } catch {
            XCTAssertTrue(error is CancellationError, "\(error)")
        }
        XCTAssertEqual(runner.hanging, 0)
        XCTAssertEqual(runner.kills, 1)
    }
}
