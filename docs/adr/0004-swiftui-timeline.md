# ADR-0004: A SwiftUI message timeline with windowed paging

* **Status:** Accepted
* **Date:** 2026-11-01
* **Relates to:** architecture §5.1, §13.7 (D1)

## Context

D1 proposes AppKit (`NSCollectionView`/`NSTableView`) for the timeline because SwiftUI lists “can
stutter with thousands of rows of varying heights”. The product owner asked for a Telegram-style
interface with rich, interactive animations: springy bubble insertion, reaction bursts,
swipe-to-reply, morphing composer controls. Those are where SwiftUI is strongest, and hosting
SwiftUI cells inside `NSTableView` would lose cross-row transitions while keeping AppKit's
complexity.

## Decision

The timeline is a SwiftUI `ScrollView` + `LazyVStack` anchored to the bottom, fed by a **windowed**
view model:

* history loads in pages of 100 (§13.7) and the window holds at most 400 rows; scrolling far back
  slides the window instead of growing it, and *Jump to latest* resets it;
* rows are `Equatable` value types keyed by message ID, so receipts and reactions re-render one
  bubble, not the list;
* images are decoded and decrypted off the main actor, BlurHash first, then thumbnail, then the
  full image;
* animations are springs of about 0.25 s and collapse to cross-fades under Reduce Motion.

The timeline is isolated behind `TimelineView`, so it can be replaced by an AppKit implementation
without touching the rest of the conversation screen.

## Consequences

If Instruments shows hitches on the §13.6 budgets (60/120 fps scrolling, < 100 ms to open a chat)
that windowing cannot fix, we switch the `TimelineView` implementation to `NSTableView` and
supersede this ADR.
