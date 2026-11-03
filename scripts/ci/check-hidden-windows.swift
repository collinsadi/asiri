// Fails unless every on-screen window of a process is left out of screen capture: sharing state
// 0, which NSWindow.sharingType .none sets (docs/adr/0007-hidden-from-screen-capture.md). Reads
// window metadata only, so it needs no Screen Recording permission.
//
//   swift scripts/ci/check-hidden-windows.swift <pid>

import CoreGraphics
import Foundation

guard CommandLine.arguments.count == 2, let pid = Int32(CommandLine.arguments[1]) else {
    print("usage: swift scripts/ci/check-hidden-windows.swift <pid>")
    exit(2)
}
let windows = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
    .filter { ($0[kCGWindowOwnerPID as String] as? Int32) == pid }
let exposed = windows.filter { ($0[kCGWindowSharingState as String] as? Int) != 0 }

if windows.isEmpty {
    print("✗ process \(pid) has no window on screen, so there's nothing to check")
    exit(1)
}
if !exposed.isEmpty {
    print("✗ \(exposed.count) of \(windows.count) on-screen windows can be captured")
    exit(1)
}
print("✓ all \(windows.count) on-screen windows are hidden from screen capture")
