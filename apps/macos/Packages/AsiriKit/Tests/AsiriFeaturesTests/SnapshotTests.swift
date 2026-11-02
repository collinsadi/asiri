import AppKit
import AsiriDesign
import AsiriDomain
@testable import AsiriFeatures
import SwiftUI
import XCTest

/// Renders the main screens, light and dark, to PNG files for review (CI uploads them as the
/// `ci-snapshots` artifact). They aren't compared pixel by pixel: these tests check that every
/// screen renders, and the images show what it looks like.
@MainActor
final class SnapshotTests: XCTestCase {
    private static let outputDir: URL = {
        let env = ProcessInfo.processInfo.environment
        let path = env["SNAPSHOT_DIR"] ?? env["TEST_RUNNER_SNAPSHOT_DIR"]
        let dir = path.map { URL(fileURLWithPath: $0) } ?? FileManager.default.temporaryDirectory.appendingPathComponent("asiri-snapshots")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }()

    private func app(registered: Bool = true, select: ChatID? = PreviewCore.adaChat) -> AppModel {
        let core = PreviewCore(registered: registered, seeded: registered, simulateNetwork: false)
        let app = AppModel(core: core, prefs: .ephemeral())
        app.start()
        if let select { app.select(select) }
        return app
    }

    /// Hosts the view in an offscreen window, lets it settle, and writes `<name>-<appearance>.png`.
    private func snapshot<V: View>(_ name: String, size: CGSize, @ViewBuilder _ view: () -> V) throws {
        for dark in [false, true] {
            let appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
            let host = NSHostingView(rootView: view()
                .frame(width: size.width, height: size.height)
                .environment(\.colorScheme, dark ? .dark : .light))
            host.frame = CGRect(origin: .zero, size: size)
            host.appearance = appearance
            let window = NSWindow(contentRect: host.frame, styleMask: [.titled, .fullSizeContentView], backing: .buffered, defer: false)
            window.appearance = appearance
            window.contentView = host
            window.orderFrontRegardless()
            host.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.9))
            host.layoutSubtreeIfNeeded()
            let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: rep)
            let png = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
            XCTAssertGreaterThan(png.count, 2_000, "\(name) rendered nothing")
            let url = Self.outputDir.appendingPathComponent("\(name)-\(dark ? "dark" : "light").png")
            try png.write(to: url)
            window.orderOut(nil)
        }
    }

    func testMainWindow() throws {
        let app = app()
        try snapshot("01-main-dm", size: CGSize(width: 1280, height: 800)) { MainView(app: app) }
    }

    func testGroupConversation() throws {
        let app = app(select: PreviewCore.squadChat)
        app.handle(.typing(chat: PreviewCore.squadChat, user: PreviewCore.chidi, active: true))
        try snapshot("02-main-group", size: CGSize(width: 1280, height: 800)) { MainView(app: app) }
    }

    func testMediaAndVoice() throws {
        let app = app(select: PreviewCore.kemiChat)
        app.showDetails = false
        try snapshot("03-voice-note", size: CGSize(width: 1000, height: 700)) { MainView(app: app) }
        let photoApp = self.app(select: PreviewCore.bayoChat)
        photoApp.showDetails = false
        try snapshot("04-photo", size: CGSize(width: 1000, height: 700)) { MainView(app: photoApp) }
    }

    func testOnboarding() throws {
        let app = app(registered: false, select: nil)
        try snapshot("05-onboarding", size: CGSize(width: 960, height: 680)) { OnboardingView(app: app) }
    }

    func testPairing() throws {
        let app = app()
        let result = PairingResult(
            emoji: [EmojiWord(glyph: "🦊", name: "Fox"), EmojiWord(glyph: "🌵", name: "Cactus"), EmojiWord(glyph: "🎈", name: "Balloon"),
                    EmojiWord(glyph: "🐢", name: "Turtle"), EmojiWord(glyph: "🍋", name: "Lemon"), EmojiWord(glyph: "🚀", name: "Rocket")],
            peerName: "Ada", peerID: PreviewCore.ada, replyCode: "ASIRI1:" + String(repeating: "Q", count: 180)
        )
        try snapshot("06-emoji-check", size: CGSize(width: 560, height: 420)) {
            EmojiConfirmation(app: app, result: result, onDone: {}).padding(24)
        }
        try snapshot("07-pairing-code", size: CGSize(width: 420, height: 420)) {
            VStack(spacing: 16) {
                QRCodeView(code: result.replyCode ?? "", size: 260)
                EmojiSequence(result.emoji, size: 24)
            }
            .padding(24)
        }
    }

    func testSettings() throws {
        let app = app()
        try snapshot("08-settings-privacy", size: CGSize(width: 560, height: 480)) { PrivacySettings(app: app) }
        try snapshot("09-settings-security", size: CGSize(width: 560, height: 480)) { SecuritySettings(app: app) }
    }

    func testCalls() throws {
        let app = app()
        let session = CallSession(callID: Data(repeating: 9, count: 16), chatID: PreviewCore.squadChat, url: "", token: "",
                                  keyBase64: "", keyIndex: 0,
                                  emoji: [EmojiWord(glyph: "🦊", name: "Fox"), EmojiWord(glyph: "🌵", name: "Cactus"),
                                          EmojiWord(glyph: "🎈", name: "Balloon"), EmojiWord(glyph: "🐢", name: "Turtle")],
                                  isVideo: true)
        let media = PreviewCallMedia(names: ["Ada Okafor", "Tolu", "Chidi"])
        media.participants = [
            CallParticipant(id: "me", name: "You", isLocal: true),
            CallParticipant(id: "a", name: "Ada Okafor", isLocal: false, isSpeaking: true),
            CallParticipant(id: "t", name: "Tolu", isLocal: false, isMuted: true),
            CallParticipant(id: "c", name: "Chidi", isLocal: false),
        ]
        media.connection = .connected
        media.isEncrypted = true
        try snapshot("10-call", size: CGSize(width: 900, height: 620)) { CallView(app: app, session: session, media: media) }
        let call = IncomingCall(callID: Data(repeating: 8, count: 16), chatID: PreviewCore.adaChat, caller: PreviewCore.ada, isVideo: false)
        try snapshot("11-incoming-call", size: CGSize(width: 480, height: 120)) {
            IncomingCallBanner(app: app, call: call).padding(20)
        }
    }

    func testEmptyStates() throws {
        let core = PreviewCore(registered: true, seeded: false, simulateNetwork: false)
        let app = AppModel(core: core, prefs: .ephemeral())
        app.start()
        try snapshot("12-empty", size: CGSize(width: 1100, height: 700)) { MainView(app: app) }
    }

    func testDesignSystem() throws {
        try snapshot("13-components", size: CGSize(width: 640, height: 460)) {
            VStack(alignment: .leading, spacing: 18) {
                HStack(spacing: 14) {
                    AsiriMark(size: 64)
                    Avatar(id: PreviewCore.ada, name: "Ada Okafor", size: 48)
                    Avatar(id: PreviewCore.tolu, name: "Tolu", size: 48)
                    Avatar(id: PreviewCore.squadChat, name: "", size: 48, isGroup: true)
                    UnreadBadge(count: 3)
                    UnreadBadge(count: 128, muted: true)
                    VerifiedBadge(.inPerson)
                    VerifiedBadge(.changed)
                }
                HStack(spacing: 16) {
                    ForEach([DeliveryStatus.pending, .sent, .delivered, .read, .failed], id: \.self) { s in
                        StatusTicks(s, tint: .secondary, readTint: Palette.accent)
                    }
                    TypingDots()
                    StatusDot(.connected)
                    StatusDot(.connecting)
                }
                HStack(spacing: 12) {
                    Button("Primary") {}.buttonStyle(PrimaryButtonStyle())
                    Button("Secondary") {}.buttonStyle(SecondaryButtonStyle())
                    CenterPill("Today")
                    CenterPill("Ada joined", icon: "person.badge.plus")
                }
                HStack(alignment: .bottom, spacing: 6) {
                    Text("Incoming bubble").padding(10).padding(.leading, 6)
                        .background(BubbleBackground(outgoing: false, tail: true, groupedAbove: false, groupedBelow: false))
                    Text("Outgoing bubble").foregroundStyle(.white).padding(10).padding(.trailing, 6)
                        .background(BubbleBackground(outgoing: true, tail: true, groupedAbove: false, groupedBelow: false))
                }
            }
            .padding(24)
            .background(ChatWallpaper())
        }
    }
}
