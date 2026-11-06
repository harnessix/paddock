import Foundation
import XCTest
@testable import PaddockKit

/// mac/launcher/Fixtures, found from this file's path.
enum Fixtures {
    static let root: URL = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // PaddockKitTests
        .deletingLastPathComponent() // Tests
        .deletingLastPathComponent() // PaddockKit
        .deletingLastPathComponent() // launcher
        .appendingPathComponent("Fixtures")

    static func scenario(_ name: String) -> URL { root.appendingPathComponent(name) }
    static func base(_ name: String) -> URL { root.appendingPathComponent("base").appendingPathComponent(name) }

    static func data(_ name: String) throws -> Data { try Data(contentsOf: base(name)) }

    static func decode<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
        try PaddockJSON.decoder.decode(T.self, from: data(name))
    }

    static func stream(_ name: String) throws -> [String] {
        try ReplayRunner.streamLines(base(name))
    }
}

/// A store wired to a replayed scenario.
@MainActor
struct Harness {
    let runner: ReplayRunner
    let service: CLIPaddockService
    let store: LauncherStore
    let clock: FakeClock

    init(_ scenario: String, start: Bool = true) async throws {
        clock = FakeClock(start: ISO.date("2026-11-06T10:00:00Z"))
        runner = try ReplayRunner.load(scenario: Fixtures.scenario(scenario), clock: clock)
        service = CLIPaddockService(runner: runner)
        store = LauncherStore(service: service, clock: clock)
        if start {
            let service = self.service
            await store.start(locate: { "/fake/paddock" }, makeService: { _ in service })
        }
    }

    var events: ReplayRunner.Feed { runner.feed(PaddockArgs.events) }

    /// Sends the lines of a stream fixture through the event stream.
    func play(_ name: String) async throws {
        for line in try Fixtures.stream(name) {
            events.send(line)
        }
        await settle()
    }

    func calls(_ prefix: [String]) -> [ReplayRunner.Call] { runner.calls(startingWith: prefix) }
}

enum ISO {
    static func date(_ text: String) -> Date {
        let formatter = ISO8601DateFormatter()
        return formatter.date(from: text)!
    }
}

/// Lets the tasks of the store run: main-actor tasks and stream deliveries.
@MainActor
func settle(_ rounds: Int = 40) async {
    for _ in 0..<rounds {
        await Task.yield()
        try? await Task.sleep(nanoseconds: 1_000_000)
    }
}

/// Waits until `condition` holds, for up to about two seconds.
@MainActor
func waitUntil(_ condition: () -> Bool, file: StaticString = #filePath, line: UInt = #line) async {
    for _ in 0..<400 {
        if condition() { return }
        await Task.yield()
        try? await Task.sleep(nanoseconds: 5_000_000)
    }
    XCTFail("condition did not become true", file: file, line: line)
}
