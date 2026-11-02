import AsiriCoreKit
import AsiriDomain
import Foundation

/// The real engine: the Rust core, reached through its UniFFI bindings, presented to the app as
/// an `AsiriDomain.CoreService`.
///
/// Both modules name some types the same way (`ChatKind`, `CoreEvent`, `Attachment`…), so every
/// such type below is written with its module: `AsiriDomain.X` is what the app sees, `AsiriCoreKit.X`
/// is what the core returns. Synchronous calls read the encrypted database and return quickly;
/// `async` calls run on the core's own runtime, never on the caller's thread.
public final class LiveCore: CoreService, @unchecked Sendable {
    private let core: AsiriCore
    private let listener: Listener

    /// Opens (or creates) the encrypted database in `dataDirectory` with the 32-byte key.
    ///
    /// - Parameters:
    ///   - releasePublicKey: The Ed25519 key that signs release manifests; `nil` turns update
    ///     checks off (development builds).
    ///   - appBuild: This build's number, compared against the newest release.
    public init(dataDirectory: URL, databaseKey: Data, releasePublicKey: Data? = nil, appBuild: UInt32) throws {
        let listener = Listener()
        let options = CoreOptions(dataDir: dataDirectory.path, releasePublicKey: releasePublicKey, appBuild: appBuild)
        do {
            core = try AsiriCore.open(options: options, dbKey: databaseKey, listener: listener)
        } catch {
            listener.finish()
            throw Convert.failure(error)
        }
        self.listener = listener
    }

    deinit {
        listener.finish()
    }

    /// The core's version, for the About screen and bug reports.
    public static var version: String { coreVersion() }

    /// Sends the core's logs to standard error (`level` is a filter such as `"info"` or
    /// `"asiri_core=debug"`). Logs never contain message content or keys.
    public static func enableLogging(_ level: String) {
        AsiriCoreKit.enableLogging(level: level)
    }

    /// Each read hands out a fresh stream and ends the previous one, so a consumer that restarts
    /// never inherits a stream its old task already cancelled. Events from before the first read
    /// are kept for it.
    public var events: AsyncStream<AsiriDomain.CoreEvent> { listener.subscribe() }

    /// Closes the connection to the server (on quit), waiting for the session to wind down.
    public func disconnect() async {
        await core.disconnect()
    }

    /// Messages saved on this Mac but not yet accepted by the server.
    public func pendingOutgoing() -> Int {
        Int((try? core.pendingOutgoing()) ?? 0)
    }

    // MARK: Profile, settings, connection

    public func profile() throws -> Profile {
        try guarded { Convert.profile(try core.profile()) }
    }

    public func setProfile(name: String, avatar: Data?) throws {
        try guarded { try core.setProfile(displayName: name, avatarJpeg: avatar) }
    }

    public func settings() throws -> AppSettings {
        try guarded { Convert.settings(try core.settings()) }
    }

    public func setSettings(_ settings: AppSettings) throws {
        try guarded { try core.setSettings(settings: Convert.settings(settings)) }
    }

    public func connectionStatus() -> ConnectionStatus {
        Convert.connection(core.connectionState())
    }

    public func connect() throws {
        try guarded { try core.connect() }
    }

    public func notifyNetworkChange() {
        core.notifyNetworkChange()
    }

    public func notifyWake(asleepSeconds: UInt64) throws {
        try guarded { try core.notifyWake(asleepSeconds: asleepSeconds) }
    }

    // MARK: Chats and messages

    public func chats() throws -> [Chat] {
        try guarded { try core.chats().map(Convert.chat) }
    }

    public func chat(_ id: ChatID) throws -> Chat {
        try guarded { Convert.chat(try core.chat(chatId: id)) }
    }

    public func messages(in chat: ChatID, before: MessageID?, limit: Int) throws -> [ChatMessage] {
        try guarded { try core.messages(chatId: chat, before: before, limit: UInt32(clamping: limit)).map(Convert.message) }
    }

    public func message(_ id: MessageID) throws -> ChatMessage {
        try guarded { Convert.message(try core.message(messageId: id)) }
    }

    public func members(of chat: ChatID) throws -> [Member] {
        try guarded { try core.members(chatId: chat).map(Convert.member) }
    }

    public func contacts() throws -> [Contact] {
        try guarded { try core.contacts().map(Convert.contact) }
    }

    public func search(_ query: String, limit: Int) throws -> [SearchHit] {
        try guarded { try core.search(query: query, limit: UInt32(clamping: limit)).map(Convert.searchHit) }
    }

    public func sendText(_ text: String, in chat: ChatID, replyTo: MessageID?) throws -> MessageID {
        try guarded { try core.sendText(chatId: chat, text: text, replyTo: replyTo) }
    }

    public func react(_ emoji: String, to message: MessageID, in chat: ChatID, remove: Bool) throws {
        try guarded { try core.react(chatId: chat, messageId: message, emoji: emoji, remove: remove) }
    }

    public func edit(_ message: MessageID, in chat: ChatID, text: String) throws {
        try guarded { try core.editMessage(chatId: chat, messageId: message, text: text) }
    }

    public func deleteForEveryone(_ message: MessageID, in chat: ChatID) throws {
        try guarded { try core.deleteForEveryone(chatId: chat, messageId: message) }
    }

    public func deleteForMe(_ message: MessageID, in chat: ChatID) throws {
        try guarded { try core.deleteForMe(chatId: chat, messageId: message) }
    }

    public func setTyping(_ active: Bool, in chat: ChatID) throws {
        try guarded { try core.setTyping(chatId: chat, active: active) }
    }

    public func markRead(_ chat: ChatID) throws {
        try guarded { try core.markRead(chatId: chat) }
    }

    public func setFlag(_ flag: AsiriDomain.ChatFlag, _ value: Bool, for chat: ChatID) throws {
        try guarded { try core.setChatFlag(chatId: chat, flag: Convert.flag(flag), value: value) }
    }

    public func setDisappearingTimer(_ seconds: UInt32, for chat: ChatID) throws {
        try guarded { try core.setDisappearingTimer(chatId: chat, seconds: seconds) }
    }

    public func updateGroup(_ chat: ChatID, name: String, description: String, avatar: Data?) throws {
        try guarded { try core.updateGroup(chatId: chat, name: name, description: description, avatarJpeg: avatar) }
    }

    public func deleteChatLocally(_ chat: ChatID) throws {
        try guarded { try core.deleteChatLocally(chatId: chat) }
    }

    public func setBlocked(_ user: UserID, _ blocked: Bool) throws {
        try guarded { try core.setBlocked(userId: user, blocked: blocked) }
    }

    public func retry(_ message: MessageID) async throws {
        try await guardedAsync { try await core.retryMessage(messageId: message) }
    }

    // MARK: Groups

    public func createChat(kind: AsiriDomain.ChatKind, name: String, members: [UserID]) async throws -> ChatID {
        try await guardedAsync { try await core.createChat(kind: Convert.chatKind(kind), name: name, members: members) }
    }

    public func addMembers(_ users: [UserID], to chat: ChatID) async throws {
        try await guardedAsync { try await core.addMembers(chatId: chat, users: users) }
    }

    public func removeMember(_ user: UserID, from chat: ChatID) async throws {
        try await guardedAsync { try await core.removeMember(chatId: chat, userId: user) }
    }

    public func setAdmin(_ user: UserID, _ admin: Bool, in chat: ChatID) async throws {
        try await guardedAsync { try await core.setAdmin(chatId: chat, userId: user, admin: admin) }
    }

    public func leave(_ chat: ChatID) async throws {
        try await guardedAsync { try await core.leaveChat(chatId: chat) }
    }

    // MARK: Pairing and verification

    public func classify(code: String) throws -> (kind: AsiriDomain.CodeKind, name: String) {
        try guarded {
            let c = try core.classifyCode(code: code)
            return (Convert.codeKind(c.kind), c.name)
        }
    }

    public func deviceOfferCode() throws -> String {
        try guarded { try core.deviceOfferCode() }
    }

    public func contactCode() throws -> String {
        try guarded { try core.contactCode() }
    }

    public func inviteDevice(offer: String) async throws -> PairingResult {
        try await guardedAsync { Convert.pairing(try await core.inviteDevice(offerCode: offer)) }
    }

    public func redeemInvitation(_ code: String) async throws -> PairingResult {
        try await guardedAsync { Convert.pairing(try await core.redeemInvitation(code: code)) }
    }

    public func addContact(card: String) async throws -> PairingResult {
        try await guardedAsync { Convert.pairing(try await core.addContact(cardCode: card)) }
    }

    public func confirmVerification(of user: UserID, inPerson: Bool) throws {
        try guarded { try core.confirmVerification(userId: user, inPerson: inPerson) }
    }

    public func pairingEmoji(for user: UserID) throws -> [EmojiWord] {
        try guarded { try core.pairingEmoji(userId: user).map(Convert.emoji) }
    }

    public func safetyNumber(for user: UserID) throws -> String {
        try guarded { try core.safetyNumber(userId: user) }
    }

    // MARK: Files

    public func sendFile(_ file: OutgoingFile, caption: String, in chat: ChatID, replyTo: MessageID?) async throws -> MessageID {
        try await guardedAsync {
            try await core.sendAttachment(chatId: chat, attachment: Convert.outgoing(file), caption: caption, replyTo: replyTo)
        }
    }

    public func download(_ message: MessageID) async throws {
        try await guardedAsync { try await core.downloadAttachment(messageId: message) }
    }

    public func attachmentData(_ message: MessageID) throws -> Data {
        try guarded { try core.attachmentData(messageId: message) }
    }

    public func exportAttachment(_ message: MessageID, to url: URL) throws {
        try guarded { try core.exportAttachment(messageId: message, destination: url.path) }
    }

    // MARK: Calls

    public func startCall(in chat: ChatID, video: Bool) async throws -> CallSession {
        try await guardedAsync { Convert.session(try await core.startCall(chatId: chat, video: video)) }
    }

    public func acceptCall(_ call: CallID) async throws -> CallSession {
        try await guardedAsync { Convert.session(try await core.acceptCall(callId: call)) }
    }

    public func declineCall(_ call: CallID) throws {
        try guarded { try core.declineCall(callId: call) }
    }

    public func endCall(_ call: CallID) throws {
        try guarded { try core.endCall(callId: call) }
    }

    public func call(_ id: CallID) throws -> CallRecord {
        try guarded { Convert.call(try core.call(callId: id)) }
    }

    public func recentCalls(limit: Int) throws -> [CallRecord] {
        try guarded { try core.recentCalls(limit: UInt32(clamping: limit)).map(Convert.call) }
    }

    public func currentCallKey(_ call: CallID) throws -> (keyBase64: String, keyIndex: UInt32) {
        try guarded {
            let key = try core.currentCallKey(callId: call)
            return (key.keyBase64, key.keyIndex)
        }
    }

    public func callParticipant(_ call: CallID, alias: String) throws -> UserID? {
        try guarded { try core.callParticipant(callId: call, alias: alias) }
    }

    // MARK: Server, updates, wipe

    public func serverStatus() async throws -> AsiriDomain.ServerStatus {
        try await guardedAsync { Convert.server(try await core.serverStatus()) }
    }

    public func revokeDevice(_ device: DeviceID) async throws {
        try await guardedAsync { try await core.revokeDevice(deviceId: device) }
    }

    public func checkForUpdate() async throws -> AsiriDomain.UpdateInfo? {
        try await guardedAsync { try await core.checkForUpdate().map(Convert.update) }
    }

    public func downloadUpdate() async throws -> URL {
        try await guardedAsync { URL(fileURLWithPath: try await core.downloadUpdate()) }
    }

    public func panicWipe() async throws {
        try await guardedAsync { try await core.panicWipe() }
        listener.finish()
    }

    // MARK: Errors

    private func guarded<T>(_ body: () throws -> T) throws -> T {
        do { return try body() } catch { throw Convert.failure(error) }
    }

    private func guardedAsync<T>(_ body: () async throws -> T) async throws -> T {
        do { return try await body() } catch { throw Convert.failure(error) }
    }
}

// MARK: - Events

/// Receives events on the core's threads and hands them to the app's stream.
private final class Listener: AsiriCoreKit.CoreListener, @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: AsyncStream<AsiriDomain.CoreEvent>.Continuation
    /// The stream created with the listener, kept until someone reads `events`.
    private var unclaimed: AsyncStream<AsiriDomain.CoreEvent>?
    private var finished = false

    init() {
        let (stream, continuation) = AsyncStream.makeStream(of: AsiriDomain.CoreEvent.self, bufferingPolicy: .unbounded)
        self.continuation = continuation
        unclaimed = stream
    }

    func subscribe() -> AsyncStream<AsiriDomain.CoreEvent> {
        lock.lock()
        defer { lock.unlock() }
        if let stream = unclaimed {
            unclaimed = nil
            return stream
        }
        continuation.finish()
        let (stream, continuation) = AsyncStream.makeStream(of: AsiriDomain.CoreEvent.self, bufferingPolicy: .unbounded)
        if finished { continuation.finish() }
        self.continuation = continuation
        return stream
    }

    func onEvent(event: AsiriCoreKit.CoreEvent) {
        let converted = Convert.event(event)
        lock.lock()
        let target = continuation
        lock.unlock()
        target.yield(converted)
    }

    func finish() {
        lock.lock()
        finished = true
        let target = continuation
        lock.unlock()
        target.finish()
    }
}

// MARK: - Conversions

/// Core records → app models, and back for the few that go the other way.
enum Convert {
    static func date(_ ms: Int64) -> Date {
        Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
    }

    static func failure(_ error: Error) -> Error {
        if error is CoreFailure || error is CancellationError { return error }
        guard let e = error as? AsiriError else {
            return CoreFailure(.internal, "Something went wrong inside Asiri. (\(error))")
        }
        switch e {
        case .Offline:
            return CoreFailure(.offline, "You're offline. Asiri will try again when you're back.")
        case let .Relay(message):
            return CoreFailure(.server, message)
        case let .Pairing(message):
            return CoreFailure(.pairing, message)
        case let .Invalid(message):
            return CoreFailure(.invalid, message)
        case .NotFound:
            return CoreFailure(.notFound, "That's no longer here.")
        case .NotRegistered:
            return CoreFailure(.notRegistered, "This Mac isn't connected to a server yet.")
        case let .NotAllowed(message):
            return CoreFailure(.notAllowed, message)
        case let .Crypto(message):
            return CoreFailure(.crypto, message)
        case let .Storage(message):
            return CoreFailure(.storage, message)
        case let .Network(message):
            return CoreFailure(.network, message)
        case let .Internal(message):
            return CoreFailure(.internal, message)
        }
    }

    static func event(_ e: AsiriCoreKit.CoreEvent) -> AsiriDomain.CoreEvent {
        switch e {
        case let .connectionChanged(state):
            return .connection(connection(state))
        case .chatsChanged:
            return .chatsChanged
        case let .chatUpdated(chat):
            return .chatUpdated(chat)
        case let .messageAdded(chat, message):
            return .messageAdded(chat: chat, message: message)
        case let .messageUpdated(chat, message):
            return .messageUpdated(chat: chat, message: message)
        case let .messageRemoved(chat, message):
            return .messageRemoved(chat: chat, message: message)
        case let .typing(chat, user, active):
            return .typing(chat: chat, user: user, active: active)
        case let .notify(chat, message, sender):
            return .notify(chat: chat, message: message, sender: sender)
        case .contactsChanged:
            return .contactsChanged
        case let .pairingCompleted(chat, user):
            return .pairingCompleted(chat: chat, user: user)
        case let .incomingCall(call):
            return .incomingCall(AsiriDomain.IncomingCall(callID: call.callId, chatID: call.chatId, caller: call.caller, isVideo: call.video))
        case let .callUpdated(call):
            return .callUpdated(call)
        case let .callKeyRotated(call, key, index):
            return .callKeyRotated(call: call, keyBase64: key, keyIndex: index)
        case let .securityWarning(chat, text):
            return .securityWarning(chat: chat, text: text)
        case let .updateAvailable(version, notes, required):
            return .updateAvailable(version: version, notes: notes, isRequired: required)
        }
    }

    static func connection(_ s: ConnectionState) -> ConnectionStatus {
        switch s {
        case .notRegistered: .notRegistered
        case .offline: .offline
        case .startingTor: .startingTor
        case .connecting: .connecting
        case .syncing: .syncing
        case .connected: .connected
        }
    }

    static func profile(_ p: CoreProfile) -> Profile {
        Profile(
            userID: p.userId, name: p.displayName, avatar: p.avatar, fingerprint: p.fingerprint,
            isRegistered: p.registered, isAdmin: p.isAdmin, relayAddress: p.relayAddress,
            isDirectTransport: p.directTransport, onionClientKey: p.onionClientKey, deviceID: p.deviceId
        )
    }

    static func settings(_ s: CoreSettings) -> AppSettings {
        AppSettings(
            readReceipts: s.readReceipts, typingIndicators: s.typingIndicators, linkPreviews: s.linkPreviews,
            defaultTimerSeconds: s.defaultTimerSeconds, autoDownloadImages: s.autoDownloadImages,
            autoDownloadMaxBytes: s.autoDownloadMaxBytes
        )
    }

    static func settings(_ s: AppSettings) -> CoreSettings {
        CoreSettings(
            readReceipts: s.readReceipts, typingIndicators: s.typingIndicators, linkPreviews: s.linkPreviews,
            defaultTimerSeconds: s.defaultTimerSeconds, autoDownloadImages: s.autoDownloadImages,
            autoDownloadMaxBytes: s.autoDownloadMaxBytes
        )
    }

    static func chatKind(_ k: AsiriCoreKit.ChatKind) -> AsiriDomain.ChatKind {
        switch k {
        case .direct: .direct
        case .group: .group
        }
    }

    static func chatKind(_ k: AsiriDomain.ChatKind) -> AsiriCoreKit.ChatKind {
        switch k {
        case .direct: .direct
        case .group: .group
        }
    }

    static func verification(_ v: AsiriCoreKit.Verification) -> AsiriDomain.Verification {
        switch v {
        case .unverified: .unverified
        case .inPerson: .inPerson
        case .remote: .remote
        case .changed: .changed
        }
    }

    static func flag(_ f: AsiriDomain.ChatFlag) -> AsiriCoreKit.ChatFlag {
        switch f {
        case .muted: .muted
        case .pinned: .pinned
        case .archived: .archived
        }
    }

    static func codeKind(_ k: AsiriCoreKit.CodeKind) -> AsiriDomain.CodeKind {
        switch k {
        case .deviceOffer: .deviceOffer
        case .invitation: .invitation
        case .contact: .contact
        }
    }

    static func messageKind(_ k: AsiriCoreKit.MessageKind) -> AsiriDomain.MessageKind {
        switch k {
        case .text: .text
        case .attachment: .attachment
        case .system: .system
        case .call: .call
        case .unsupported: .unsupported
        }
    }

    static func status(_ s: MessageStatus) -> DeliveryStatus {
        switch s {
        case .pending: .pending
        case .sent: .sent
        case .delivered: .delivered
        case .read: .read
        case .failed: .failed
        case .received: .received
        case .seen: .seen
        }
    }

    static func transfer(_ s: AttachmentState) -> TransferState {
        switch s {
        case .remote: .remote
        case .downloading: .downloading
        case .local: .local
        case .failed: .failed
        case .uploading: .uploading
        }
    }

    static func attachment(_ a: AsiriCoreKit.Attachment) -> AsiriDomain.Attachment {
        AsiriDomain.Attachment(
            size: a.size, mime: a.mime, fileName: a.fileName, width: Int(a.width), height: Int(a.height),
            durationMs: Int(a.durationMs), thumbnail: a.thumbnail, blurhash: a.blurhash, isVoiceNote: a.voiceNote,
            waveform: a.waveform.map { [UInt8]($0) } ?? [], state: transfer(a.state), progress: a.progress
        )
    }

    static func outgoing(_ f: OutgoingFile) -> OutgoingAttachment {
        OutgoingAttachment(
            data: f.data, mime: f.mime, fileName: f.fileName, width: UInt32(clamping: f.width),
            height: UInt32(clamping: f.height), durationMs: UInt32(clamping: f.durationMs), thumbnail: f.thumbnail,
            blurhash: f.blurhash, voiceNote: f.isVoiceNote, waveform: f.waveform.isEmpty ? nil : Data(f.waveform)
        )
    }

    static func message(_ m: CoreMessage) -> ChatMessage {
        ChatMessage(
            id: m.messageId, chatID: m.chatId, sender: m.sender, isOutgoing: m.outgoing, kind: messageKind(m.kind),
            body: m.body, replyTo: m.replyTo, sentAt: date(m.sentAtMs), receivedAt: date(m.receivedAtMs),
            sortKey: m.sortMs, status: status(m.status), isEdited: m.edited, isDeleted: m.deleted,
            expiresAt: m.expiresAtMs.map(date), attachment: m.attachment.map(attachment),
            reactions: m.reactions.map { AsiriDomain.Reaction(emoji: $0.emoji, count: Int($0.count), mine: $0.mine) }
        )
    }

    static func chat(_ c: CoreChat) -> Chat {
        Chat(
            id: c.chatId, kind: chatKind(c.kind), title: c.title, description: c.description, avatar: c.avatar,
            peer: c.peer, lastMessage: c.lastMessage.map(message), lastSenderName: c.lastSenderName,
            verification: c.verification.map(verification), memberCount: Int(c.memberCount), isAdmin: c.isAdmin,
            unread: Int(c.unread), isMuted: c.muted, isPinned: c.pinned, isArchived: c.archived,
            timerSeconds: c.timerSeconds, hasLeft: c.left, lastActivity: date(c.lastActivityMs)
        )
    }

    static func contact(_ c: CoreContact) -> Contact {
        Contact(id: c.userId, name: c.displayName, avatar: c.avatar, verification: verification(c.verification), isBlocked: c.blocked)
    }

    static func member(_ m: CoreMember) -> Member {
        Member(contact: contact(m.contact), isAdmin: m.isAdmin, isMe: m.isMe)
    }

    static func searchHit(_ h: CoreSearchHit) -> SearchHit {
        SearchHit(chatID: h.chatId, messageID: h.messageId, snippet: h.snippet, sentAt: date(h.sentAtMs))
    }

    static func emoji(_ e: Emoji) -> EmojiWord {
        EmojiWord(glyph: e.glyph, name: e.name)
    }

    static func pairing(_ p: CorePairingStep) -> PairingResult {
        PairingResult(emoji: p.emoji.map(emoji), peerName: p.peerName, peerID: p.peerUserId, replyCode: p.replyCode, chatID: p.chatId)
    }

    static func session(_ s: CoreCallSession) -> CallSession {
        CallSession(
            callID: s.callId, chatID: s.chatId, url: s.url, token: s.token, keyBase64: s.keyBase64,
            keyIndex: s.keyIndex, emoji: s.emoji.map(emoji), isVideo: s.video
        )
    }

    static func phase(_ s: CallState) -> CallPhase {
        switch s {
        case .ringing: .ringing
        case .active: .active
        case .ended: .ended
        }
    }

    static func call(_ c: CoreCall) -> CallRecord {
        CallRecord(
            id: c.callId, chatID: c.chatId, isVideo: c.video, isOutgoing: c.outgoing, caller: c.caller,
            phase: phase(c.state), startedAt: date(c.startedMs), answeredAt: c.answeredMs.map(date),
            endedAt: c.endedMs.map(date), isJoined: c.joined
        )
    }

    static func server(_ s: AsiriCoreKit.ServerStatus) -> AsiriDomain.ServerStatus {
        AsiriDomain.ServerStatus(
            devices: s.devices.map { d in
                let state: AsiriDomain.ServerDevice.State = switch d.state {
                case 1: .active
                case 2: .revoked
                default: .pending
                }
                return AsiriDomain.ServerDevice(id: d.deviceId, state: state, isAdmin: d.isAdmin)
            },
            invitePolicy: s.invitePolicy, storedBytes: s.storedBlobBytes, queuedEnvelopes: s.queuedEnvelopes
        )
    }

    static func update(_ u: AsiriCoreKit.UpdateInfo) -> AsiriDomain.UpdateInfo {
        AsiriDomain.UpdateInfo(version: u.version, build: u.build, notes: u.notes, size: u.size, isRequired: u.required)
    }
}
