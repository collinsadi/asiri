import AsiriDesign
import AsiriDomain
@testable import AsiriFeatures
import AsiriPlatform
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers
import XCTest

@MainActor
final class ConversationTests: XCTestCase {
    private func makeApp() -> AppModel {
        let app = AppModel(core: PreviewCore(simulateNetwork: false), prefs: .ephemeral())
        app.start()
        return app
    }

    func testStartsReadyWithPinnedChatFirst() {
        let app = makeApp()
        XCTAssertEqual(app.phase, .ready)
        XCTAssertEqual(app.chats.first?.id, PreviewCore.adaChat, "pinned chats come first")
        XCTAssertGreaterThan(app.totalUnread, 0)
    }

    func testUnregisteredMacShowsOnboarding() {
        let app = AppModel(core: PreviewCore(registered: false, seeded: false, simulateNetwork: false), prefs: .ephemeral())
        app.start()
        XCTAssertEqual(app.phase, .onboarding)
    }

    func testOpeningAChatMarksItReadAndBuildsGroupedRows() {
        let app = makeApp()
        app.select(PreviewCore.adaChat)
        let c = app.conversation(for: PreviewCore.adaChat)
        XCTAssertEqual(app.chats.first { $0.id == PreviewCore.adaChat }?.unread, 0)
        let bubbles = c.rows.compactMap { row -> (ChatMessage, BubbleGrouping)? in
            if case let .message(m, g) = row, m.isBubble { return (m, g) }
            return nil
        }
        XCTAssertFalse(bubbles.isEmpty)
        // Collins's two messages from the same minute form one group with one tail.
        let mine = bubbles.filter { $0.0.isOutgoing && $0.0.body.contains("📸") || $0.0.body.contains("lagoon") }
        XCTAssertEqual(mine.count, 2)
        XCTAssertTrue(mine[0].1.groupedBelow)
        XCTAssertTrue(mine[1].1.groupedAbove)
        XCTAssertTrue(mine[1].1.hasTail)
        XCTAssertFalse(mine[0].1.hasTail)
        XCTAssertTrue(c.rows.contains { if case .day = $0 { return true } else { return false } })
    }

    func testSendingClearsTheDraftAndAppendsAPendingBubble() async throws {
        let app = makeApp()
        app.select(PreviewCore.adaChat)
        let c = app.conversation(for: PreviewCore.adaChat)
        c.draft = "  See you soon!  "
        XCTAssertTrue(c.send())
        XCTAssertEqual(c.draft, "")
        XCTAssertEqual(c.sentCounter, 1)
        // The core's event arrives on the main actor.
        try await Task.sleep(for: .milliseconds(100))
        let last = try XCTUnwrap(c.messages.last)
        XCTAssertEqual(last.body, "See you soon!")
        XCTAssertTrue(last.isOutgoing)
        XCTAssertEqual(last.status, .pending)
        XCTAssertFalse(c.send(), "an empty draft sends nothing")
    }

    func testUpArrowEditsTheLastOwnMessage() {
        let app = makeApp()
        app.select(PreviewCore.adaChat)
        let c = app.conversation(for: PreviewCore.adaChat)
        XCTAssertTrue(c.editLast())
        XCTAssertEqual(c.editing?.body, "Yes! Usual place.")
        XCTAssertEqual(c.draft, "Yes! Usual place.")
        c.cancelCompose()
        XCTAssertNil(c.editing)
        XCTAssertEqual(c.draft, "")
    }

    func testReactionsToggle() async throws {
        let app = makeApp()
        app.select(PreviewCore.adaChat)
        let c = app.conversation(for: PreviewCore.adaChat)
        let target = try XCTUnwrap(c.messages.last { !$0.isOutgoing && $0.isBubble })
        c.toggleReaction("🔥", on: target)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(c.message(target.id)?.reactions.first { $0.emoji == "🔥" }?.mine, true)
        c.toggleReaction("🔥", on: try XCTUnwrap(c.message(target.id)))
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertNil(c.message(target.id)?.reactions.first { $0.emoji == "🔥" })
    }

    func testFiltersAndSearch() {
        let app = makeApp()
        app.filter = .groups
        XCTAssertTrue(app.visibleChats.allSatisfy { $0.kind == .group })
        app.filter = .unread
        XCTAssertTrue(app.visibleChats.allSatisfy { $0.unread > 0 })
        app.filter = .all
        app.searchText = "jollof"
        app.runSearch()
        XCTAssertEqual(app.searchHits.count, 1)
        XCTAssertTrue(app.searchHits[0].snippet.contains("[jollof]"))
    }

    func testTypingShowsInTheHeader() {
        let app = makeApp()
        app.select(PreviewCore.squadChat)
        let c = app.conversation(for: PreviewCore.squadChat)
        app.handle(.typing(chat: PreviewCore.squadChat, user: PreviewCore.tolu, active: true))
        XCTAssertEqual(c.subtitle, "Tolu is typing…")
        app.handle(.typing(chat: PreviewCore.squadChat, user: PreviewCore.tolu, active: false))
        XCTAssertEqual(c.subtitle, "4 members")
    }

    func testEmojiOnlyMessagesAreDetected() {
        XCTAssertTrue(BubbleView.isEmojiOnly("🔥"))
        XCTAssertTrue(BubbleView.isEmojiOnly("😂😂"))
        XCTAssertFalse(BubbleView.isEmojiOnly("ok 👍"))
        XCTAssertFalse(BubbleView.isEmojiOnly("12"))
        XCTAssertFalse(BubbleView.isEmojiOnly("😂😂😂😂"))
    }
}

final class FormattingTests: XCTestCase {
    func testDurationsAndSizes() {
        XCTAssertEqual(Formatting.duration(ms: 14_000), "0:14")
        XCTAssertEqual(Formatting.duration(ms: 723_000), "12:03")
        XCTAssertEqual(Formatting.duration(ms: 3_765_000), "1:02:45")
        XCTAssertEqual(Formatting.grouped("ABCDEFGH"), "ABCD EFGH")
        XCTAssertFalse(Formatting.bytes(4_200_000).isEmpty)
    }

    func testDaySeparators() {
        let now = Date()
        XCTAssertEqual(Formatting.daySeparator(now, now: now), "Today")
        let yesterday = Calendar.current.date(byAdding: .day, value: -1, to: now)!
        XCTAssertEqual(Formatting.daySeparator(yesterday, now: now), "Yesterday")
    }

    func testTimerLabels() {
        XCTAssertEqual(DisappearingTimer.label(0), "Off")
        XCTAssertEqual(DisappearingTimer.label(604_800), "1 week")
    }
}

final class ImagingTests: XCTestCase {
    func testBlurHashRoundTrip() throws {
        // A 16×16 gradient.
        let w = 16, h = 16
        var pixels = [UInt8](repeating: 255, count: w * h * 4)
        for y in 0 ..< h {
            for x in 0 ..< w {
                let o = (y * w + x) * 4
                pixels[o] = UInt8(x * 16)
                pixels[o + 1] = UInt8(y * 16)
                pixels[o + 2] = 128
            }
        }
        let provider = try XCTUnwrap(CGDataProvider(data: Data(pixels) as CFData))
        let image = try XCTUnwrap(CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4,
                                          space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                                          provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent))
        let hash = BlurHashEncoder.encode(image)
        XCTAssertEqual(hash.count, 4 + 2 * 4 * 3)
        XCTAssertNotNil(BlurHash.image(hash))
        XCTAssertNil(BlurHash.image("nope"))
    }

    func testQRCodesRender() {
        XCTAssertNotNil(QRCode.image(for: "ASIRI1:" + String(repeating: "x", count: 150)))
    }

    func testWaveformBuckets() {
        let bars = VoiceRecorder.bucket([0, 0.5, 1, 0.25], into: 8)
        XCTAssertEqual(bars.count, 8)
        XCTAssertTrue(bars.allSatisfy { $0 >= 12 })
    }

    func testPhotosLoseTheirMetadata() throws {
        // A JPEG with a GPS location must come out without it.
        let w = 64, h = 48
        let ctx = try XCTUnwrap(CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                          space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue))
        ctx.setFillColor(CGColor(red: 0.3, green: 0.4, blue: 0.9, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        let image = try XCTUnwrap(ctx.makeImage())
        let data = NSMutableData()
        let dest = try XCTUnwrap(CGImageDestinationCreateWithData(data as CFMutableData, "public.jpeg" as CFString, 1, nil))
        let gps: [CFString: Any] = [kCGImagePropertyGPSLatitude: 6.45, kCGImagePropertyGPSLongitude: 3.39]
        CGImageDestinationAddImage(dest, image, [kCGImagePropertyGPSDictionary: gps] as CFDictionary)
        XCTAssertTrue(CGImageDestinationFinalize(dest))
        let file = try MediaPrep.prepare(data: data as Data, type: .jpeg, fileName: "IMG_1.HEIC")
        XCTAssertEqual(file.mime, "image/jpeg")
        XCTAssertEqual(file.fileName, "IMG_1.jpg")
        XCTAssertEqual(file.width, w)
        XCTAssertNotNil(file.thumbnail)
        XCTAssertFalse(file.blurhash.isEmpty)
        let source = try XCTUnwrap(CGImageSourceCreateWithData(file.data as CFData, nil))
        let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any] ?? [:]
        XCTAssertNil(props[kCGImagePropertyGPSDictionary], "the location survived")
    }
}

@MainActor
final class LockingTests: XCTestCase {
    func testAutoLockAfterTheChosenTimeInTheBackground() {
        let prefs = Preferences.ephemeral()
        prefs.appLockEnabled = true
        prefs.autoLockMinutes = 5
        let app = AppModel(core: PreviewCore(simulateNetwork: false), prefs: prefs)
        app.start()
        let start = Date()

        app.didResignActive(at: start)
        XCTAssertFalse(app.isActive)
        app.didBecomeActive(at: start.addingTimeInterval(60))
        XCTAssertEqual(app.phase, .ready, "a minute away doesn't lock")
        XCTAssertTrue(app.isActive)

        app.didResignActive(at: start)
        app.didBecomeActive(at: start.addingTimeInterval(301))
        XCTAssertEqual(app.phase, .locked, "five minutes away locks before anything shows")
        app.unlocked()
        XCTAssertEqual(app.phase, .ready)
    }

    func testNothingLocksWithoutALockMethod() throws {
        try XCTSkipIf(PINStore().hasPIN, "this Mac has an Asiri PIN set")
        let prefs = Preferences.ephemeral()
        prefs.autoLockMinutes = 1
        let app = AppModel(core: PreviewCore(simulateNetwork: false), prefs: prefs)
        app.start()
        let start = Date()
        app.didResignActive(at: start)
        app.didBecomeActive(at: start.addingTimeInterval(3600))
        XCTAssertEqual(app.phase, .ready)
    }
}

final class CallLayoutTests: XCTestCase {
    func testTilesFitTheWindowAtTheirLargest() {
        let one = TileLayout(count: 1, in: CGSize(width: 800, height: 500))
        XCTAssertEqual(one.columns, 1)
        XCTAssertEqual(one.size, CGSize(width: 800, height: 500))

        let four = TileLayout(count: 4, in: CGSize(width: 880, height: 540))
        XCTAssertEqual(four.columns, 2)
        XCTAssertLessThanOrEqual(2 * four.size.height + TileLayout.spacing, 540)
        XCTAssertLessThanOrEqual(2 * four.size.width + TileLayout.spacing, 880)

        // A wide, short window puts everyone in one row.
        let wide = TileLayout(count: 3, in: CGSize(width: 1600, height: 320))
        XCTAssertEqual(wide.columns, 3)
        XCTAssertEqual(TileLayout(count: 0, in: .zero).size, .zero)
    }
}
