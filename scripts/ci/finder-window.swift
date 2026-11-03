// Prints "<id> <width> <height>" for Finder's frontmost window on screen, so CI can check the
// window a disk image opens and photograph it with `screencapture -l <id>`. Reads window
// metadata only, so it needs no Screen Recording permission.
//
//   swift scripts/ci/finder-window.swift

import CoreGraphics
import Foundation

let windows = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
    .filter { ($0[kCGWindowOwnerName as String] as? String) == "Finder" && ($0[kCGWindowLayer as String] as? Int) == 0 }
guard let window = windows.first,
      let id = window[kCGWindowNumber as String] as? Int,
      let bounds = window[kCGWindowBounds as String] as? [String: Any],
      let width = bounds["Width"] as? Double, let height = bounds["Height"] as? Double
else {
    FileHandle.standardError.write(Data("✗ Finder has no window on screen\n".utf8))
    exit(1)
}
print(id, Int(width), Int(height))
