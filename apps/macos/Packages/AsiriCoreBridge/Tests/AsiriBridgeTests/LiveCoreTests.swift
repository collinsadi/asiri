import AsiriBridge
import AsiriDomain
import Foundation
import XCTest

/// The Swift side of the bridge, against the real Rust core: types, errors, async calls and
/// events all cross UniFFI here. With `ASIRI_RELAY_CODE` set to a development relay's admin code
/// (`asiri-sim relay` prints one), two cores also pair and chat through that relay, as two Macs
/// would. CI runs both.
final class LiveCoreTests: XCTestCase {
    private var directories: [URL] = []

    override func tearDown() {
        for dir in directories { try? FileManager.default.removeItem(at: dir) }
        directories = []
        super.tearDown()
    }

    private func makeCore(_ name: String) throws -> LiveCore {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("asiri-bridge-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        directories.append(dir)
        let key = Data((0 ..< 32).map { _ in UInt8.random(in: .min ... .max) })
        let core = try LiveCore(dataDirectory: dir, databaseKey: key, appBuild: 1)
        try core.setProfile(name: name, avatar: nil)
        return core
    }

    func testANewMacHasKeysButNoServerYet() throws {
        let core = try makeCore("Ada")
        let profile = try core.profile()
        XCTAssertEqual(profile.name, "Ada")
        XCTAssertEqual(profile.userID.count, 16)
        XCTAssertFalse(profile.isRegistered)
        XCTAssertEqual(core.connectionStatus(), .notRegistered)
        XCTAssertTrue(try core.chats().isEmpty)

        let offer = try core.deviceOfferCode()
        XCTAssertTrue(offer.hasPrefix("ASIRI1:"))
        XCTAssertEqual(try core.classify(code: offer).kind, .deviceOffer)

        var settings = try core.settings()
        settings.readReceipts = false
        settings.defaultTimerSeconds = 3_600
        try core.setSettings(settings)
        XCTAssertEqual(try core.settings(), settings)
    }

    func testCoreErrorsArriveAsPlainLanguageFailures() throws {
        let core = try makeCore("Ada")
        XCTAssertThrowsError(try core.classify(code: "hello")) { error in
            XCTAssertTrue(error is CoreFailure, "got \(type(of: error))")
            XCTAssertFalse((error as? CoreFailure)?.message.isEmpty ?? true)
        }
        XCTAssertThrowsError(try core.chat(Data(repeating: 7, count: 32))) { error in
            XCTAssertEqual((error as? CoreFailure)?.kind, .notFound)
        }
        XCTAssertThrowsError(try core.sendText("hi", in: Data(repeating: 7, count: 32), replyTo: nil))
    }

    func testTwoMacsPairAndChatThroughARealRelay() async throws {
        guard let code = ProcessInfo.processInfo.environment["ASIRI_RELAY_CODE"], !code.isEmpty else {
            throw XCTSkip("set ASIRI_RELAY_CODE to the admin code a development relay prints (asiri-sim relay)")
        }
        let ada = try makeCore("Ada")
        let ben = try makeCore("Ben")
        let benEvents = EventLog(ben.events)

        // Ada sets up the server; Ben pairs with her in person.
        _ = try await ada.redeemInvitation(code)
        XCTAssertTrue(try ada.profile().isRegistered)
        XCTAssertTrue(try ada.profile().isAdmin)
        let invitation = try await ada.inviteDevice(offer: try ben.deviceOfferCode())
        let joined = try await ben.redeemInvitation(try XCTUnwrap(invitation.replyCode))
        XCTAssertEqual(joined.emoji, invitation.emoji, "both screens show the same emoji")
        XCTAssertEqual(joined.peerID, try ada.profile().userID)
        _ = try await eventually("Ada to connect") { ada.connectionStatus() == .connected ? true : nil }

        let dm = try await eventually("Ada's DM with Ben") { try ada.chats().first { $0.kind == .direct }?.id }
        _ = try await eventually("Ben to have the DM") { try ben.chats().first { $0.id == dm } }

        // A message, a reaction and a read receipt cross the relay, end to end encrypted.
        let text = "Hello from Swift 👋"
        let sent = try ada.sendText(text, in: dm, replyTo: nil)
        let received = try await eventually("Ben to receive it") {
            try ben.messages(in: dm, before: nil, limit: 50).first { $0.body == text }
        }
        XCTAssertFalse(received.isOutgoing)
        XCTAssertEqual(received.sender, try ada.profile().userID)
        XCTAssertTrue(benEvents.contains { event in
            if case let .messageAdded(chat, message) = event { return chat == dm && message == received.id }
            return false
        }, "the core told the app about the new message")

        try ben.react("❤️", to: received.id, in: dm, remove: false)
        _ = try await eventually("Ada to see Ben's reaction") {
            try ada.message(sent).reactions.first { $0.emoji == "❤️" }
        }
        try ben.markRead(dm)
        _ = try await eventually("Ada to see it read") { try ada.message(sent).status == .read ? true : nil }

        // Files go up and come down intact.
        let bytes = Data("a small file, encrypted before it leaves".utf8)
        let file = OutgoingFile(data: bytes, mime: "text/plain", fileName: "note.txt")
        let fileID = try await ada.sendFile(file, caption: "", in: dm, replyTo: nil)
        _ = try await eventually("Ben to get the file") { try ben.message(fileID) }
        try await ben.download(fileID)
        XCTAssertEqual(try ben.attachmentData(fileID), bytes)
    }

    /// Polls until `check` returns a value (errors count as "not yet").
    private func eventually<T>(_ what: String, timeout: TimeInterval = 30, _ check: () throws -> T?) async throws -> T {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if let value = try? check() { return value }
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTFail("timed out waiting for \(what)")
        throw CancellationError()
    }
}

/// Collects a core's events in the background.
private final class EventLog: @unchecked Sendable {
    private let lock = NSLock()
    private var events: [CoreEvent] = []
    private var task: Task<Void, Never>?

    init(_ stream: AsyncStream<CoreEvent>) {
        task = Task { [weak self] in
            for await event in stream {
                self?.append(event)
            }
        }
    }

    deinit { task?.cancel() }

    private func append(_ event: CoreEvent) {
        lock.lock()
        events.append(event)
        lock.unlock()
    }

    func contains(where match: (CoreEvent) -> Bool) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return events.contains(where: match)
    }
}
