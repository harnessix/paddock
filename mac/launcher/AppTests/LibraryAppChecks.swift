import AppKit
import PaddockKit
import XCTest
@testable import PaddockLauncher

/// The app side of the library: the Dock drop type, the session buttons,
/// the drop queue and the texts of the new screens through the catalog.
@MainActor
final class LibraryAppChecks: XCTestCase {
    // An app dropped on the Dock icon reaches the launcher, and the launcher
    // never becomes the default opener of apps.
    func testDockAcceptsAppBundlesAsAlternate() throws {
        let types = try XCTUnwrap(Bundle.main.object(forInfoDictionaryKey: "CFBundleDocumentTypes") as? [[String: Any]])
        let app = try XCTUnwrap(types.first { ($0["LSItemContentTypes"] as? [String])?.contains("com.apple.application-bundle") == true })
        XCTAssertEqual(app["LSHandlerRank"] as? String, "Alternate")
        XCTAssertEqual(app["CFBundleTypeRole"] as? String, "Viewer")
    }

    func testSessionButtonsPerState() {
        XCTAssertEqual(SessionButtons.of(.stopped, canSuspend: true), [.start])
        XCTAssertEqual(SessionButtons.of(.saved(since: nil), canSuspend: false), [.start])
        XCTAssertEqual(SessionButtons.of(.suspended(reason: nil), canSuspend: true), [.resume, .stop])
        XCTAssertEqual(SessionButtons.of(.running(since: nil, windows: 1), canSuspend: false), [.open, .stop])
        XCTAssertEqual(SessionButtons.of(.running(since: nil, windows: 1), canSuspend: true), [.open, .suspend, .stop])
        XCTAssertEqual(SessionButtons.of(.starting(.booting), canSuspend: true), [.stop])
        XCTAssertEqual(SessionButtons.of(.stopping, canSuspend: true), [])
        XCTAssertEqual(SessionButtons.of(.blocked(reason: "x", code: nil), canSuspend: true), [])
        XCTAssertEqual(SessionButtons.of(.error(reason: "x"), canSuspend: true), [.start])
    }

    func testMenuTitlesResolve() {
        for button in [SessionButtons.start, .resume, .open, .suspend, .stop] {
            XCTAssertNotNil(SessionMenuTitle.of(button))
        }
        XCTAssertEqual(LibraryCopy.notAnApp.localized, "Only apps can be added. Drag an app from your Applications folder.")
        XCTAssertEqual(LibraryCopy.removed("Sample Mail", freed: 3_200_000_000, trashBytes: 0).localized, "Sample Mail removed. 3.2 GB freed.")
        XCTAssertEqual(SelfInstall.question.localized, "Move paddock for Mac to your Applications folder?")
    }

    // A drop without an app queues nothing and says why.
    func testDropOfAFileIsRefused() {
        let center = AddCenter()
        center.receive(["/Users/demo/notes.txt"])
        XCTAssertTrue(center.queue.isEmpty)
        XCTAssertEqual(center.refusal, LibraryCopy.notAnApp)
    }

    // The test host runs from Xcode's build folder: no move question.
    func testNoMoveQuestionForABuild() {
        XCTAssertFalse(SelfInstall.shouldOffer(SelfInstaller.place, declined: false) && SelfInstaller.bundlePath.contains("/Build/"))
    }
}
