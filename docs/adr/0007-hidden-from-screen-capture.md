# ADR-0007: Hide Asiri from screen capture

* **Status:** Accepted
* **Date:** 2026-11-03
* **Relates to:** architecture §13.1, §14.1

## Context

§14.1 said that macOS can't reliably block screenshots, and left disappearing messages and trust
between friends as the only defense. But macOS does let an app take a window out of what other
apps capture: a window whose `sharingType` is `.none` is left out of screenshots, screen
recordings and screen sharing (the screenshot tools, QuickTime, ⌘⇧5, and ScreenCaptureKit, which
Zoom, Meet, FaceTime and Asiri's own calls use). The window isn't blacked out; it isn't in the
picture at all. For a messenger whose chats are meant to stay between friends, a screenshot or a
forgotten screen share is the easiest way for them to leak.

## Decision

Every Asiri window has `sharingType = .none`, always, with no setting to turn it off:

* The windows Asiri builds (the main window, calls, Settings, the menu bar panel, and every sheet
  and popover) are shielded by `captureShielded()` when their content is attached, before their
  first frame.
* `CaptureShield.start()`, called before launch finishes, shields every other window as it
  appears: alerts, menus and anything AppKit creates.

Asiri's own screen sharing therefore never shows Asiri.

## Consequences

* Asiri's chats, photos and calls don't appear in screenshots, recordings or screen shares made
  on the Mac, by the user or by software running as them.
* Users can't screenshot a chat to share it, which is the point.
* It isn't a guarantee. A camera pointed at the screen still sees everything, macOS decides which
  capture paths honor the setting, and an attacker with control of the Mac has other ways in.
  Disappearing messages and trust between friends remain the defense beyond it. Check it again
  on each new macOS release, and keep notification previews off (the default), because macOS
  draws notification banners itself.
* `CaptureShieldTests` checks that windows are shielded before they're shown and when they appear
  later.
