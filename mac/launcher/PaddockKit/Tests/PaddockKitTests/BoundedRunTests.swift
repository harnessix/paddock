import Foundation
import XCTest
@testable import PaddockKit
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// The bounded run of `ProcessRunner` with real child processes: the fake
/// paddock script replays the protocol scenarios, small shell scripts
/// hang, flood or leave a grandchild behind.
final class BoundedRunTests: XCTestCase {
    var state: URL!

    static let fake = Fixtures.root.deletingLastPathComponent()
        .appendingPathComponent("FakePaddock").appendingPathComponent("fake-paddock").path

    override func setUpWithError() throws {
        state = FileManager.default.temporaryDirectory.appendingPathComponent("bounded-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: state, withIntermediateDirectories: true)
        // The child inherits the environment of the test process.
        setenv("PADDOCK_FAKE_STATE", state.path, 1)
    }

    override func tearDownWithError() throws {
        unsetenv("PADDOCK_FAKE_SCENARIO")
        unsetenv("PADDOCK_FAKE_STATE")
        try? FileManager.default.removeItem(at: state)
    }

    func scenario(_ name: String) -> CLIPaddockService {
        setenv("PADDOCK_FAKE_SCENARIO", Fixtures.scenario(name).path, 1)
        return CLIPaddockService(runner: ProcessRunner(executable: BoundedRunTests.fake))
    }

    /// A shell script in the scratch folder.
    func script(_ body: String) throws -> String {
        let url = state.appendingPathComponent("script-\(UUID().uuidString)")
        try ("#!/bin/sh\n" + body + "\n").write(to: url, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        return url.path
    }

    func waitForFile(_ url: URL) async throws -> String {
        for _ in 0..<400 {
            if let text = try? String(contentsOf: url, encoding: .utf8), text.hasSuffix("\n") {
                return text.trimmingCharacters(in: .whitespacesAndNewlines)
            }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        XCTFail("no \(url.lastPathComponent)")
        return ""
    }

    func isGone(_ pid: pid_t) -> Bool {
        kill(pid, 0) == -1 && errno == ESRCH
    }

    // MARK: The verdict with real processes

    func testAGood022BuildPasses() async {
        let verdict = await LauncherStore.verdict(of: scenario("protocol-good"))
        XCTAssertEqual(verdict, .ok(version: "0.2.2"))
    }

    func testTheReleased022WithoutTheQueryIsRefused() async {
        let verdict = await LauncherStore.verdict(of: scenario("protocol-none"))
        XCTAssertEqual(verdict, .incompatible(version: "0.2.2", reason: .noAnswer(status: 2)))
    }

    func testABadAnswerIsRefused() async {
        let verdict = await LauncherStore.verdict(of: scenario("protocol-bad-json"))
        XCTAssertEqual(verdict, .incompatible(version: "0.2.2", reason: .badAnswer))
    }

    func testAMissingFeatureIsRefused() async {
        let verdict = await LauncherStore.verdict(of: scenario("protocol-missing-feature"))
        XCTAssertEqual(verdict, .incompatible(version: "0.2.2", reason: .missingFeatures(["events-v1"])))
    }

    func testAnOversizedAnswerIsRefused() async {
        let verdict = await LauncherStore.verdict(of: scenario("protocol-oversized"))
        XCTAssertEqual(verdict, .incompatible(version: "0.2.2", reason: .tooLarge))
    }

    // MARK: The limits

    // The fake hangs in `sleep 3600`: the limit kills it, and the run
    // ends long before the hang would.
    func testAHangIsKilledAtTheTimeLimit() async throws {
        _ = scenario("protocol-hang")
        let runner = ProcessRunner(executable: BoundedRunTests.fake)
        let began = Date()
        do {
            _ = try await runner.run(PaddockArgs.launcherProtocol, limits: RunLimits(seconds: 2, maxBytes: 1024))
            XCTFail("expected a time-out")
        } catch {
            XCTAssertEqual(error as? CommandError, .timedOut)
        }
        XCTAssertLessThan(Date().timeIntervalSince(began), 6)
        let pid = pid_t(try await waitForFile(state.appendingPathComponent("hang.pid")))!
        XCTAssertTrue(isGone(pid), "the hanging child \(pid) still runs")
    }

    func testACancelKillsTheChild() async throws {
        _ = scenario("protocol-hang")
        let runner = ProcessRunner(executable: BoundedRunTests.fake)
        let task = Task {
            try await runner.run(PaddockArgs.launcherProtocol, limits: RunLimits(seconds: 60, maxBytes: 1024))
        }
        let pid = pid_t(try await waitForFile(state.appendingPathComponent("hang.pid")))!
        XCTAssertFalse(isGone(pid))
        let began = Date()
        task.cancel()
        do {
            _ = try await task.value
            XCTFail("expected a cancel")
        } catch {
            XCTAssertTrue(error is CancellationError, "\(error)")
        }
        XCTAssertLessThan(Date().timeIntervalSince(began), 5)
        XCTAssertTrue(isGone(pid), "the cancelled child \(pid) still runs")
    }

    // A cancel before the start: no child starts.
    func testACancelBeforeTheStartStartsNothing() async throws {
        let marker = state.appendingPathComponent("started")
        let path = try script("touch '\(marker.path)'")
        let runner = ProcessRunner(executable: path)
        let task = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await runner.run(["x"], limits: RunLimits(seconds: 5, maxBytes: 1024))
        }
        do {
            _ = try await task.value
            XCTFail("expected a cancel")
        } catch {
            XCTAssertTrue(error is CancellationError, "\(error)")
        }
        try await Task.sleep(nanoseconds: 300_000_000)
        XCTAssertFalse(FileManager.default.fileExists(atPath: marker.path))
    }

    func testAFloodIsCutAtTheOutputLimit() async throws {
        let pidFile = state.appendingPathComponent("flood.pid")
        let path = try script("echo $$ > '\(pidFile.path)'\nwhile :; do printf '%01024d' 0; done")
        let runner = ProcessRunner(executable: path)
        do {
            _ = try await runner.run(["x"], limits: RunLimits(seconds: 30, maxBytes: 16 * 1024))
            XCTFail("expected an oversized output")
        } catch {
            XCTAssertEqual(error as? CommandError, .tooLarge)
        }
        let pid = pid_t(try await waitForFile(pidFile))!
        XCTAssertTrue(isGone(pid), "the flooding child \(pid) still runs")
    }

    func testAFloodOnStderrIsCutToo() async throws {
        let path = try script("while :; do printf '%01024d' 0 >&2; done")
        do {
            _ = try await ProcessRunner(executable: path).run(["x"], limits: RunLimits(seconds: 30, maxBytes: 4096))
            XCTFail("expected an oversized output")
        } catch {
            XCTAssertEqual(error as? CommandError, .tooLarge)
        }
    }

    // The child exits, but a grandchild keeps the pipe open: the time limit
    // still ends the run.
    func testAGrandchildThatHoldsThePipeCannotHoldTheRun() async throws {
        let path = try script("sleep 5 &\necho early")
        let began = Date()
        do {
            _ = try await ProcessRunner(executable: path).run(["x"], limits: RunLimits(seconds: 1, maxBytes: 1024))
            XCTFail("expected a time-out")
        } catch {
            XCTAssertEqual(error as? CommandError, .timedOut)
        }
        XCTAssertLessThan(Date().timeIntervalSince(began), 4)
    }

    func testAQuickAnswerIsReturnedWhole() async throws {
        let path = try script("printf 'out'\nprintf 'err' >&2\nexit 3")
        let out = try await ProcessRunner(executable: path).run(["x"], limits: RunLimits(seconds: 5, maxBytes: 1024))
        XCTAssertEqual(out, CommandOutput(status: 3, stdout: Data("out".utf8), stderr: "err"))
    }

    func testAMissingBinaryIsNotFound() async {
        let runner = ProcessRunner(executable: state.appendingPathComponent("none").path)
        do {
            _ = try await runner.run(["x"], limits: RunLimits(seconds: 5, maxBytes: 1024))
            XCTFail("expected not found")
        } catch {
            XCTAssertEqual(error as? CommandError, .notFound)
        }
        let verdict = await LauncherStore.verdict(of: CLIPaddockService(runner: runner))
        XCTAssertEqual(verdict, .unreadable(output: ""))
    }
}
