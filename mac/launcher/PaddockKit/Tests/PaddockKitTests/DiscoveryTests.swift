import XCTest
@testable import PaddockKit

/// Binary discovery, the version gate and the prerequisites checklist.
final class DiscoveryTests: XCTestCase {
    let home = "/Users/demo"

    func locator(existing: Set<String>, env: [String: String] = [:], debug: Bool = true,
                 brew: String? = nil) -> BinaryLocator {
        BinaryLocator(environment: env, home: home, isDebugBuild: debug,
                      isExecutable: { existing.contains($0) }, brewPrefix: { _ in brew })
    }

    // Each of the five locations alone is found.
    func testEachLocationIsFound() {
        let cases: [(Set<String>, [String: String], String?, String)] = [
            (["/tmp/fake"], ["PADDOCK_BIN": "/tmp/fake"], nil, "/tmp/fake"),
            (["/opt/homebrew/bin/paddock"], [:], nil, "/opt/homebrew/bin/paddock"),
            (["/usr/local/bin/paddock"], [:], nil, "/usr/local/bin/paddock"),
            (["/Users/demo/.local/bin/paddock"], [:], nil, "/Users/demo/.local/bin/paddock"),
            (["/usr/local/bin/brew", "/usr/local/Cellar/x/bin/paddock"], [:], "/usr/local/Cellar/x\n",
             "/usr/local/Cellar/x/bin/paddock"),
        ]
        for (existing, env, brew, expected) in cases {
            XCTAssertEqual(locator(existing: existing, env: env, brew: brew).locate(), expected)
        }
    }

    // With all present, the first in order wins, one by one.
    func testFirstExistingWinsInOrder() {
        var all: Set<String> = ["/tmp/fake", "/opt/homebrew/bin/paddock", "/usr/local/bin/paddock",
                                "/Users/demo/.local/bin/paddock", "/opt/homebrew/bin/brew", "/brew/bin/paddock"]
        let env = ["PADDOCK_BIN": "/tmp/fake"]
        let order = ["/tmp/fake", "/opt/homebrew/bin/paddock", "/usr/local/bin/paddock",
                     "/Users/demo/.local/bin/paddock", "/brew/bin/paddock"]
        for expected in order {
            XCTAssertEqual(locator(existing: all, env: env, brew: "/brew").locate(), expected)
            all.remove(expected)
        }
        XCTAssertNil(locator(existing: all, env: env, brew: "/brew").locate())
    }

    func testMissingGivesNil() {
        XCTAssertNil(locator(existing: []).locate())
    }

    func testReleaseBuildIgnoresPaddockBin() {
        let found = locator(existing: ["/tmp/fake"], env: ["PADDOCK_BIN": "/tmp/fake"], debug: false).locate()
        XCTAssertNil(found)
    }

    func testSearchRootHidesRealInstallInDebug() {
        let l = locator(existing: ["/opt/homebrew/bin/paddock"], env: ["PADDOCK_LAUNCHER_SEARCH_ROOT": "/nowhere"])
        XCTAssertNil(l.locate())
    }

    @MainActor
    func testMissingBinaryShowsInstallCommand() async {
        let store = LauncherStore(service: nil)
        await store.start(locate: { nil }, makeService: { _ in fatalError("not called") })
        XCTAssertEqual(store.binary, .missing)
        XCTAssertTrue(store.needsOnboarding)
        let rows = Prerequisites.rows(doctor: nil, binary: nil, binaryMissing: true)
        let row = rows.first { $0.kind == .paddock }!
        XCTAssertEqual(row.command, "brew install harnessix/tap/paddock")
        XCTAssertEqual(row.result.english, "Not installed")
        XCTAssertEqual(LauncherError.notInstalled.primary.title?.english, "Check Again")
    }

    // MARK: Version gate

    func testVersionGate() {
        let gate = VersionGate(minimum: (0, 3), supportedMajor: 0)
        XCTAssertEqual(gate.check(versionOutput: "paddock 0.2.1\n"), .tooOld(have: "0.2.1", need: "0.3"))
        XCTAssertEqual(gate.check(versionOutput: "paddock 1.0.0"), .tooNew(have: "1.0.0"))
        XCTAssertEqual(gate.check(versionOutput: "paddock 0.3.0"), .ok(version: "0.3.0"))
        XCTAssertEqual(gate.check(versionOutput: "paddock 0.4.2"), .ok(version: "0.4.2"))
        XCTAssertEqual(gate.check(versionOutput: "v0.3"), .ok(version: "0.3.0"))
        XCTAssertEqual(gate.check(versionOutput: "garbage"), .unreadable(output: "garbage"))
    }

    func testVersionGateTitles() {
        let gate = VersionGate.current
        XCTAssertEqual(gate.error(for: .tooOld(have: "0.2.1", need: "0.3"))?.title?.english, "Update paddock")
        XCTAssertEqual(gate.error(for: .tooNew(have: "1.0.0"))?.title?.english, "Update paddock for Mac")
        XCTAssertNil(gate.error(for: .ok(version: "0.3.0")))
        XCTAssertEqual(LauncherError.tooOld(have: "0.2.1", need: "0.3").body?.english,
                       "This version of paddock for Mac needs paddock 0.3 or newer. You have 0.2.1.")
    }

    @MainActor
    func testOldAndNewVersionStopTheStart() async throws {
        let old = try await Harness("old-version")
        XCTAssertEqual(old.store.binary, .found(path: "/fake/paddock", verdict: .tooOld(have: "0.2.1", need: "0.3")))
        XCTAssertTrue(old.calls(["app", "list"]).isEmpty)
        let new = try await Harness("new-version")
        XCTAssertEqual(new.store.binary, .found(path: "/fake/paddock", verdict: .tooNew(have: "1.0.0")))
        let ok = try await Harness("ready")
        XCTAssertTrue(ok.store.binary.isUsable)
    }

    // MARK: Prerequisites

    func rows(_ doctor: String) throws -> [PrereqRow] {
        Prerequisites.rows(doctor: try Fixtures.decode(DoctorJSON.self, doctor), binary: .ok(version: "0.4.2"),
                           binaryMissing: false)
    }

    func testAppleSiliconPasses() throws {
        let r = try rows("doctor-ok.json")
        XCTAssertTrue(Prerequisites.canContinue(r))
        XCTAssertEqual(r.map(\.kind), [.chip, .macos, .paddock, .space, .memory])
        XCTAssertEqual(r[0].result.english, "This Mac has an M-series chip")
        XCTAssertEqual(r[2].result.english, "paddock 0.4.2 found")
        XCTAssertEqual(r[3].result.english, "184.0 GB free, 40.0 GB needed")
        XCTAssertEqual(r[4].result.english, "16 GB; each app gets up to 8 GB")
        XCTAssertEqual(r[0].symbol, "checkmark.circle")
    }

    func testIntelIsADeadEnd() throws {
        let r = try rows("doctor-intel.json")
        XCTAssertFalse(Prerequisites.canContinue(r))
        XCTAssertTrue(Prerequisites.isDeadEnd(r))
        XCTAssertEqual(r[0].actions, [.quit])
        XCTAssertEqual(r[0].result.english, "paddock app mode needs a Mac with Apple silicon.")
        XCTAssertEqual(r[0].symbol, "xmark.circle")
    }

    func testOldMacOSOffersUpdate() throws {
        let r = try rows("doctor-old-macos.json")
        XCTAssertFalse(Prerequisites.canContinue(r))
        XCTAssertEqual(r[1].actions, [.updateMacOS])
    }

    func testLowDiskOffersManageStorage() throws {
        let r = try rows("doctor-low-disk.json")
        XCTAssertFalse(Prerequisites.canContinue(r))
        XCTAssertEqual(r[3].actions, [.manageStorage, .checkAgain])
        XCTAssertEqual(r[3].result.english, "Needs 40.0 GB, 12.0 GB free. Free up space, then click Check again.")
    }

    func testLowMemoryIsOnlyAWarning() throws {
        let r = try rows("doctor-low-memory.json")
        XCTAssertTrue(Prerequisites.canContinue(r))
        XCTAssertEqual(r[4].status, .warning)
        XCTAssertEqual(r[4].result.english, "8 GB; apps get 4 GB each, which is the minimum.")
    }

    func testOldPaddockRowOffersUpgrade() {
        let r = Prerequisites.rows(doctor: nil, binary: .tooOld(have: "0.2.1", need: "0.3"), binaryMissing: false)
        let row = r.first { $0.kind == .paddock }!
        XCTAssertEqual(row.command, "brew upgrade paddock")
        XCTAssertEqual(row.status, .fail)
    }
}
