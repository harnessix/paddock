import AppKit
import PaddockKit
import SwiftUI
import XCTest
@testable import PaddockLauncher

/// The polish layer: icon cache and sizes, the spoken row line, the colors
/// of status and disk, the step symbols and the menu icon.
@MainActor
final class PolishChecks: XCTestCase {
    // A real bundle gives its own icon, cached; a missing bundle gives nil,
    // and the view then draws the generic icon, never an empty square.
    func testIconCacheByPath() throws {
        let finder = "/System/Library/CoreServices/Finder.app"
        try XCTSkipUnless(FileManager.default.fileExists(atPath: finder), "no Finder bundle")
        let first = try XCTUnwrap(AppIconCache.icon(finder))
        XCTAssertTrue(first === AppIconCache.icon(finder), "the second call reads the cache")
        XCTAssertNil(AppIconCache.icon("/nonexistent/Gone.app"))
        XCTAssertGreaterThan(AppIconCache.generic.size.width, 0)
    }

    func testIconSizes() {
        XCTAssertEqual(IconSize.header, 64)
        XCTAssertEqual(IconSize.question, 32)
        XCTAssertEqual(IconSize.sidebar, 24)
        XCTAssertEqual(IconSize.menu, 16)
    }

    // `Sample Notes, Running, 3 windows`: the word first, the detail after,
    // and no word twice.
    func testRowSpeech() {
        let now = Date()
        XCTAssertEqual(RowSpeech.of("Sample Notes", .of(.running(since: nil, windows: 3), now: now)),
                       "Sample Notes, Running, 3 windows")
        XCTAssertEqual(RowSpeech.of("Sample Notes", .of(.stopped, now: now)), "Sample Notes, Stopped")
        XCTAssertEqual(RowSpeech.of("Sample Notes", .of(.error(reason: "The VM stopped"), now: now)),
                       "Sample Notes, Error: The VM stopped")
        XCTAssertEqual(RowSpeech.of("Sample Notes", .of(.saved(since: now.addingTimeInterval(-7200)), now: now)),
                       "Sample Notes, Ready, saved 2 h ago")
    }

    // Color is a data role: green Running, orange Error, red Blocked, gray
    // for the rest. Every one comes with a word.
    func testStatusColors() {
        let now = Date()
        XCTAssertEqual(StatusPresentation.of(.running(since: nil, windows: 1), now: now).dotColor, .green)
        XCTAssertEqual(StatusPresentation.of(.error(reason: "x"), now: now).dotColor, .orange)
        XCTAssertEqual(StatusPresentation.of(.blocked(reason: "x", code: nil), now: now).dotColor, .red)
        XCTAssertEqual(StatusPresentation.of(.stopped, now: now).dotColor, .gray)
        XCTAssertEqual(MenuAppIcon.dotColor(.of(.running(since: nil, windows: 1), now: now)), .systemGreen)
        XCTAssertEqual(MenuAppIcon.dotColor(.of(.stopped, now: now)), .systemGray)
        for state: AppState in [.stopped, .running(since: nil, windows: 1), .error(reason: "x"),
                                .blocked(reason: "x", code: nil), .stopping, .starting(.booting)] {
            XCTAssertFalse(StatusPresentation.of(state, now: now).word.english.isEmpty)
        }
    }

    func testDiskColorsAreSystemColors() {
        XCTAssertEqual(DiskBar.color(.appCopy), .blue)
        XCTAssertEqual(DiskBar.color(.data), .teal)
        XCTAssertEqual(DiskBar.color(.savedState), .gray)
    }

    func testOnboardingStepSymbols() {
        XCTAssertEqual(OnboardingSteps.symbol(1, current: 2), "checkmark.circle.fill")
        XCTAssertEqual(OnboardingSteps.symbol(2, current: 2), "circle.inset.filled")
        XCTAssertEqual(OnboardingSteps.symbol(3, current: 2), "circle")
        for name in ["checkmark.circle.fill", "circle.inset.filled", "circle", "questionmark.circle.fill",
                     "shield.lefthalf.filled", "arrow.clockwise", "ellipsis"] {
            XCTAssertNotNil(NSImage(systemSymbolName: name, accessibilityDescription: nil), name)
        }
    }

    // The menu icon is 16 pt and speaks the state; a missing bundle still
    // draws.
    func testMenuIcon() {
        let image = MenuAppIcon.image(path: "/nonexistent/Gone.app", presentation: .of(.stopped, now: Date()))
        XCTAssertEqual(image.size, NSSize(width: 16, height: 16))
        XCTAssertEqual(image.accessibilityDescription, "Stopped")
        XCTAssertNotNil(image.cgImage(forProposedRect: nil, context: nil, hints: nil))
    }

    func testPrimarySessionButton() {
        XCTAssertEqual(SessionButtons.primary(of: [.open, .suspend, .stop]), .open)
        XCTAssertEqual(SessionButtons.primary(of: [.start]), .start)
        XCTAssertNil(SessionButtons.primary(of: [.stop]))
    }

    // PADDOCK_LAUNCHER_SNAPSHOT_SELECT: `Questions` in any case, an app by
    // key, name or title, a second instance by its own key; else nothing.
    func testSnapshotSelect() {
        let apps = [SnapshotDump.Row(key: "Sample Notes", name: "Sample Notes", title: "Sample Notes"),
                    SnapshotDump.Row(key: "Sample Notes--2", name: "Sample Notes", title: "Sample Notes 2"),
                    SnapshotDump.Row(key: "Sample Mail", name: "Sample Mail", title: "Mail")]
        XCTAssertEqual(SnapshotDump.sidebarItem(for: "Questions", apps: apps), .questions)
        XCTAssertEqual(SnapshotDump.sidebarItem(for: "questions", apps: apps), .questions)
        XCTAssertEqual(SnapshotDump.sidebarItem(for: "Sample Notes", apps: apps), .app("Sample Notes"))
        XCTAssertEqual(SnapshotDump.sidebarItem(for: "Sample Notes--2", apps: apps), .app("Sample Notes--2"))
        XCTAssertEqual(SnapshotDump.sidebarItem(for: "Sample Notes 2", apps: apps), .app("Sample Notes--2"))
        XCTAssertEqual(SnapshotDump.sidebarItem(for: "Mail", apps: apps), .app("Sample Mail"))
        XCTAssertEqual(SnapshotDump.sidebarItem(for: "sample mail", apps: apps), .app("Sample Mail"))
        XCTAssertNil(SnapshotDump.sidebarItem(for: "Notebook", apps: apps))
    }
}
