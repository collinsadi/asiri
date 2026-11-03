import AppKit
import AsiriPlatform
import SwiftUI
import XCTest

/// Asiri stays out of screenshots, screen recordings and screen sharing (§14.1).
@MainActor
final class CaptureShieldTests: XCTestCase {
    private func makeWindow() -> NSWindow {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 240, height: 120),
                              styleMask: [.titled], backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        return window
    }

    /// A window is shielded as soon as a shielded view is put in it: before it's ever shown, so
    /// not even its first frame can be captured.
    func testAShieldedViewHidesItsWindowBeforeItIsShown() {
        let window = makeWindow()
        XCTAssertNotEqual(window.sharingType, .none, "windows are capturable unless told otherwise")
        let host = NSHostingView(rootView: Text("A secret").captureShielded())
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(window.sharingType, .none)
        XCTAssertFalse(window.isVisible)
    }

    /// Windows Asiri doesn't build itself (sheets, alerts, popovers, menus) are shielded as
    /// they appear.
    func testTheWatcherShieldsWindowsThatOpenLater() {
        CaptureShield.start()
        let window = makeWindow()
        window.orderFront(nil)
        defer { window.close() }
        NotificationCenter.default.post(name: NSApplication.didUpdateNotification, object: NSApp)
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.05))
        XCTAssertEqual(window.sharingType, .none)
    }
}
