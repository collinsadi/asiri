import AppKit
import SwiftUI

/// Keeps Asiri out of screenshots, screen recordings and screen sharing (architecture §14.1).
///
/// macOS leaves a window whose `sharingType` is `.none` out of everything other apps capture:
/// the screenshot tools, QuickTime and ⌘⇧5 recordings, and screen sharing in Zoom, Meet,
/// FaceTime or Asiri's own calls. The window isn't blacked out; it simply isn't in the picture.
/// Asiri's own windows are shielded before their first frame (``SwiftUI/View/captureShielded()``),
/// and the watcher started here catches every other window as it appears: sheets, popovers,
/// alerts, menus and the menu bar panel.
@MainActor
public enum CaptureShield {
    private static var observers: [NSObjectProtocol] = []

    /// Shields every window from now on. Call once, before the first window appears.
    public static func start() {
        guard observers.isEmpty else { return }
        let center = NotificationCenter.default
        let triggers: [Notification.Name] = [
            NSApplication.didUpdateNotification, // after every event, so new windows are seen at once
            NSWindow.didBecomeKeyNotification,
            NSWindow.didBecomeMainNotification,
            NSMenu.didBeginTrackingNotification,
        ]
        observers = triggers.map { name in
            center.addObserver(forName: name, object: nil, queue: .main) { _ in
                MainActor.assumeIsolated { shieldAll() }
            }
        }
        shieldAll()
    }

    /// Shields one window.
    public static func shield(_ window: NSWindow) {
        if window.sharingType != .none { window.sharingType = .none }
    }

    /// Shields every window the app has right now.
    public static func shieldAll() {
        for window in NSApplication.shared.windows { shield(window) }
    }
}

public extension View {
    /// Keeps the window this view is shown in out of screen captures, from its very first frame.
    func captureShielded() -> some View {
        background(CaptureShieldAnchor())
    }
}

/// An invisible view that shields whichever window it's put into, before that window is drawn.
private struct CaptureShieldAnchor: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView { AnchorView() }
    func updateNSView(_ nsView: NSView, context: Context) {}

    final class AnchorView: NSView {
        override func viewWillMove(toWindow newWindow: NSWindow?) {
            super.viewWillMove(toWindow: newWindow)
            if let newWindow { CaptureShield.shield(newWindow) }
        }

        // Never in the way of a click.
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
    }
}
