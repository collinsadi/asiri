import Foundation

/// An in-memory `CoreService` with a believable cast of friends and chats. It powers SwiftUI
/// previews, the snapshot tests and the app's demo mode, and it behaves like the real engine:
/// messages progress from pending to read, friends type and reply, reactions and edits work.
public final class PreviewCore: CoreService, @unchecked Sendable {
    public let events: AsyncStream<CoreEvent>
    private let continuation: AsyncStream<CoreEvent>.Continuation
    private let lock = NSLock()
    private let simulate: Bool

    private var me: Profile
    private var settingsValue = AppSettings()
    private var contactsByID: [UserID: Contact] = [:]
    private var chatsByID: [ChatID: Chat] = [:]
    private var messagesByChat: [ChatID: [ChatMessage]] = [:]
    private var membersByChat: [ChatID: [UserID]] = [:]
    private var adminsByChat: [ChatID: Set<UserID>] = [:]
    private var calls: [CallID: CallRecord] = [:]
    private var status: ConnectionStatus

    // MARK: Cast

    public static let collins = PreviewCore.id(0xC0)
    public static let ada = PreviewCore.id(0xA1)
    public static let bayo = PreviewCore.id(0xB2)
    public static let tolu = PreviewCore.id(0x70)
    public static let kemi = PreviewCore.id(0x4E)
    public static let chidi = PreviewCore.id(0xC1)

    public static let adaChat = PreviewCore.chatID(0x01)
    public static let squadChat = PreviewCore.chatID(0x02)
    public static let bayoChat = PreviewCore.chatID(0x03)
    public static let kemiChat = PreviewCore.chatID(0x04)
    public static let familyChat = PreviewCore.chatID(0x05)

    /// - Parameters:
    ///   - registered: whether this Mac is already paired with a server (false shows onboarding).
    ///   - seeded: start with the demo chats.
    ///   - simulateNetwork: deliver, read and reply on timers like real friends would.
    ///   - now: the clock the seeded history is anchored to (fixed in snapshot tests).
    public init(registered: Bool = true, seeded: Bool = true, simulateNetwork: Bool = true, now: Date = Date()) {
        (events, continuation) = AsyncStream.makeStream(of: CoreEvent.self, bufferingPolicy: .unbounded)
        simulate = simulateNetwork
        me = Profile(
            userID: Self.collins,
            name: registered ? "Collins" : "",
            fingerprint: "4F2A 91C3 77D0 1B8E 5A6C 0D2F 93B4 E781",
            isRegistered: registered,
            isAdmin: registered,
            relayAddress: registered ? "ksfz7l2…onion" : nil,
            onionClientKey: "descriptor:x25519:KXQ7H4R6WMTE3L2Q5OBJFZ7NVGDIYUAKXC2P6S4ZTHRQ3MBLJ5WA",
            deviceID: registered ? Self.id(0xD0) : nil
        )
        status = registered ? .connected : .notRegistered
        if seeded { seed(now: now) }
    }

    deinit {
        continuation.finish()
    }

    private static func id(_ byte: UInt8) -> Data { Data(repeating: byte, count: 16) }
    private static func chatID(_ byte: UInt8) -> Data { Data(repeating: byte, count: 32) }
    private static func randomID(_ count: Int = 16) -> Data {
        Data((0 ..< count).map { _ in UInt8.random(in: 0 ... 255) })
    }

    private func emit(_ event: CoreEvent) {
        continuation.yield(event)
    }

    private func locked<T>(_ body: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try body()
    }

    // MARK: Seed

    private func seed(now: Date) {
        let people: [(UserID, String, Verification)] = [
            (Self.ada, "Ada Okafor", .inPerson),
            (Self.bayo, "Bayo Adeyemi", .inPerson),
            (Self.tolu, "Tolu", .remote),
            (Self.kemi, "Kemi", .inPerson),
            (Self.chidi, "Chidi", .unverified),
        ]
        for (id, name, v) in people {
            contactsByID[id] = Contact(id: id, name: name, verification: v)
        }
        let minute: TimeInterval = 60
        func at(_ minutesAgo: Double) -> Date { now.addingTimeInterval(-minutesAgo * minute) }

        addChat(Self.adaChat, kind: .direct, title: "Ada Okafor", peer: Self.ada, members: [Self.collins, Self.ada], pinned: true)
        addChat(Self.squadChat, kind: .group, title: "The Squad", members: [Self.collins, Self.ada, Self.tolu, Self.chidi], admins: [Self.collins])
        addChat(Self.bayoChat, kind: .direct, title: "Bayo Adeyemi", peer: Self.bayo, members: [Self.collins, Self.bayo])
        addChat(Self.kemiChat, kind: .direct, title: "Kemi", peer: Self.kemi, members: [Self.collins, Self.kemi], timer: 604_800)
        addChat(Self.familyChat, kind: .group, title: "Family", members: [Self.collins, Self.kemi, Self.bayo], admins: [Self.kemi], muted: true)

        system(Self.adaChat, "Messages in this chat are end-to-end encrypted. Only the two of you can read them.", at(60 * 26))
        text(Self.adaChat, from: Self.ada, "Hey! Did you get the photos from Saturday?", at(60 * 25), status: .seen)
        text(Self.adaChat, from: Self.collins, "Yes, they came out great 📸", at(60 * 25 - 2), status: .read)
        text(Self.adaChat, from: Self.collins, "The one at the lagoon is my favourite", at(60 * 25 - 1.5), status: .read,
             reactions: [Reaction(emoji: "❤️", count: 1, mine: false)])
        let question = text(Self.adaChat, from: Self.ada, "Are we still on for 6?", at(9), status: .seen)
        text(Self.adaChat, from: Self.collins, "Yes! Usual place.", at(6), status: .read, replyTo: question)
        text(Self.adaChat, from: Self.ada, "Perfect. I'll bring the book I told you about", at(4), status: .seen)
        text(Self.adaChat, from: Self.ada, "See you at 6 ✨", at(3), status: .received)

        system(Self.squadChat, "You created “The Squad”", at(60 * 72))
        text(Self.squadChat, from: Self.tolu, "Who's free on Friday?", at(60 * 3), status: .seen)
        text(Self.squadChat, from: Self.chidi, "Me! What's the plan?", at(60 * 3 - 4), status: .seen)
        text(Self.squadChat, from: Self.collins, "Board games at mine. I'll make jollof", at(60 * 2), status: .read,
             reactions: [Reaction(emoji: "🔥", count: 2, mine: false), Reaction(emoji: "😂", count: 1, mine: false)])
        text(Self.squadChat, from: Self.tolu, "lol you always say that", at(32), status: .received)
        text(Self.squadChat, from: Self.ada, "I'm in 🙌", at(31), status: .received)

        let photo = Attachment(size: 1_840_000, mime: "image/jpeg", fileName: "IMG_2041.jpg", width: 1600, height: 1200,
                               blurhash: "LEHV6nWB2yk8pyo0adR*.7kCMdnj", state: .local, progress: 1)
        attachment(Self.bayoChat, from: Self.bayo, photo, caption: "The new studio is finally ready", at(60 * 30))
        text(Self.bayoChat, from: Self.collins, "Wow, that light! Congrats 🎉", at(60 * 29), status: .delivered)

        let voice = Attachment(size: 48_000, mime: "audio/mp4", fileName: "voice.m4a", durationMs: 14_000, isVoiceNote: true,
                               waveform: (0 ..< 48).map { i in UInt8(40 + (i * 37 % 160)) }, state: .local, progress: 1)
        system(Self.kemiChat, "You set disappearing messages to 1 week", at(60 * 50))
        attachment(Self.kemiChat, from: Self.kemi, voice, caption: "", at(60 * 49))
        text(Self.kemiChat, from: Self.collins, "Will call you tonight", at(60 * 48), status: .read)

        text(Self.familyChat, from: Self.kemi, "Mummy says Sunday lunch is at 2", at(60 * 24 * 3), status: .seen)

        if var c = chatsByID[Self.adaChat] { c.unread = 2; chatsByID[Self.adaChat] = c }
        if var c = chatsByID[Self.squadChat] { c.unread = 2; chatsByID[Self.squadChat] = c }
    }

    private func addChat(_ id: ChatID, kind: ChatKind, title: String, peer: UserID? = nil, members: [UserID],
                         admins: Set<UserID>? = nil, pinned: Bool = false, muted: Bool = false, timer: UInt32 = 0) {
        let verification = peer.flatMap { contactsByID[$0]?.verification }
        chatsByID[id] = Chat(id: id, kind: kind, title: title, peer: peer, verification: verification, memberCount: members.count,
                             isAdmin: kind == .direct || (admins ?? []).contains(Self.collins), isMuted: muted, isPinned: pinned,
                             timerSeconds: timer, lastActivity: .distantPast)
        membersByChat[id] = members
        adminsByChat[id] = kind == .direct ? Set(members) : (admins ?? [])
        messagesByChat[id] = []
    }

    @discardableResult
    private func text(_ chat: ChatID, from sender: UserID, _ body: String, _ at: Date, status: DeliveryStatus,
                      replyTo: MessageID? = nil, reactions: [Reaction] = []) -> MessageID {
        let m = ChatMessage(id: Self.randomID(), chatID: chat, sender: sender, isOutgoing: sender == Self.collins, kind: .text,
                            body: body, replyTo: replyTo, sentAt: at, status: status, reactions: reactions)
        append(m)
        return m.id
    }

    private func attachment(_ chat: ChatID, from sender: UserID, _ a: Attachment, caption: String, _ at: Date) {
        append(ChatMessage(id: Self.randomID(), chatID: chat, sender: sender, isOutgoing: sender == Self.collins, kind: .attachment,
                           body: caption, sentAt: at, status: sender == Self.collins ? .read : .seen, attachment: a))
    }

    private func system(_ chat: ChatID, _ body: String, _ at: Date) {
        append(ChatMessage(id: Self.randomID(), chatID: chat, sender: Self.collins, isOutgoing: false, kind: .system,
                           body: body, sentAt: at, status: .received))
    }

    private func append(_ m: ChatMessage) {
        messagesByChat[m.chatID, default: []].append(m)
        messagesByChat[m.chatID]?.sort { $0.sortKey < $1.sortKey }
        guard var c = chatsByID[m.chatID] else { return }
        c.lastActivity = max(c.lastActivity, m.sentAt)
        chatsByID[m.chatID] = c
    }

    private func summarize(_ c: Chat) -> Chat {
        var chat = c
        let last = messagesByChat[c.id]?.last
        chat.lastMessage = last
        if let last, c.kind == .group, !last.isOutgoing, last.kind != .system {
            chat.lastSenderName = contactsByID[last.sender].map { $0.name.components(separatedBy: " ").first ?? $0.name }
        }
        return chat
    }

    private func notFound() -> CoreFailure { CoreFailure(.notFound, "Not found") }

    // MARK: Profile, settings, connection

    public func profile() throws -> Profile { locked { me } }

    public func setProfile(name: String, avatar: Data?) throws {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed.count <= 64 else { throw CoreFailure(.invalid, "Choose a name between 1 and 64 characters") }
        locked {
            me.name = trimmed
            me.avatar = avatar
        }
    }

    public func settings() throws -> AppSettings { locked { settingsValue } }
    public func setSettings(_ settings: AppSettings) throws { locked { settingsValue = settings } }
    public func connectionStatus() -> ConnectionStatus { locked { status } }

    public func connect() throws {
        guard locked({ me.isRegistered }) else { throw CoreFailure(.notRegistered, "This Mac isn't connected to a server yet") }
        setStatus(.connected)
    }

    private func setStatus(_ s: ConnectionStatus) {
        locked { status = s }
        emit(.connection(s))
    }

    public func notifyNetworkChange() {}
    public func notifyWake(asleepSeconds: UInt64) throws {}

    // MARK: Chats and messages

    public func chats() throws -> [Chat] {
        locked {
            chatsByID.values.map(summarize).sorted {
                if $0.isPinned != $1.isPinned { return $0.isPinned }
                return $0.lastActivity > $1.lastActivity
            }
        }
    }

    public func chat(_ id: ChatID) throws -> Chat {
        try locked {
            guard let c = chatsByID[id] else { throw notFound() }
            return summarize(c)
        }
    }

    public func messages(in chat: ChatID, before: MessageID?, limit: Int) throws -> [ChatMessage] {
        locked {
            let all = messagesByChat[chat] ?? []
            let end = before.flatMap { b in all.firstIndex { $0.id == b } } ?? all.count
            return Array(all[max(0, end - limit) ..< end])
        }
    }

    public func message(_ id: MessageID) throws -> ChatMessage {
        try locked {
            for list in messagesByChat.values {
                if let m = list.first(where: { $0.id == id }) { return m }
            }
            throw notFound()
        }
    }

    public func members(of chat: ChatID) throws -> [Member] {
        locked {
            let admins = adminsByChat[chat] ?? []
            return (membersByChat[chat] ?? []).map { id in
                let contact = id == Self.collins
                    ? Contact(id: id, name: me.name, avatar: me.avatar, verification: .inPerson)
                    : contactsByID[id] ?? Contact(id: id, name: "")
                return Member(contact: contact, isAdmin: admins.contains(id), isMe: id == Self.collins)
            }
            .sorted { $0.isMe != $1.isMe ? $0.isMe : $0.contact.name < $1.contact.name }
        }
    }

    public func contacts() throws -> [Contact] {
        locked { contactsByID.values.sorted { $0.name < $1.name } }
    }

    public func search(_ query: String, limit: Int) throws -> [SearchHit] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return [] }
        let all: [ChatMessage] = locked { messagesByChat.values.flatMap { $0 } }
        let matches = all.filter { (m: ChatMessage) -> Bool in
            let searchable = m.kind == .text || m.kind == .attachment
            return searchable && !m.isDeleted && m.body.lowercased().contains(q)
        }
        let newest = matches.sorted { (a: ChatMessage, b: ChatMessage) -> Bool in a.sentAt > b.sentAt }
        return newest.prefix(limit).map { (m: ChatMessage) -> SearchHit in
            var snippet = m.body
            if let r = snippet.range(of: q, options: .caseInsensitive) {
                snippet.replaceSubrange(r, with: "[\(snippet[r])]")
            }
            return SearchHit(chatID: m.chatID, messageID: m.id, snippet: snippet, sentAt: m.sentAt)
        }
    }

    public func sendText(_ text: String, in chat: ChatID, replyTo: MessageID?) throws -> MessageID {
        let body = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { throw CoreFailure(.invalid, "Message is empty") }
        let m = try locked { () throws -> ChatMessage in
            guard let c = chatsByID[chat] else { throw notFound() }
            guard !c.hasLeft else { throw CoreFailure(.notAllowed, "You're no longer in this chat") }
            let expires = c.timerSeconds > 0 ? Date().addingTimeInterval(TimeInterval(c.timerSeconds)) : nil
            let m = ChatMessage(id: Self.randomID(), chatID: chat, sender: Self.collins, isOutgoing: true, body: body,
                                replyTo: replyTo, sentAt: Date(), status: .pending, expiresAt: expires)
            append(m)
            return m
        }
        emit(.messageAdded(chat: chat, message: m.id))
        emit(.chatsChanged)
        if simulate { progress(m) }
        return m.id
    }

    private func setStatus(of id: MessageID, in chat: ChatID, _ s: DeliveryStatus) {
        locked {
            guard let i = messagesByChat[chat]?.firstIndex(where: { $0.id == id }) else { return }
            messagesByChat[chat]?[i].status = s
        }
        emit(.messageUpdated(chat: chat, message: id))
    }

    /// Pending → sent → delivered → read, and sometimes a reply.
    private func progress(_ m: ChatMessage) {
        let chat = m.chatID
        Task.detached { [weak self] in
            try? await Task.sleep(for: .milliseconds(350))
            self?.setStatus(of: m.id, in: chat, .sent)
            try? await Task.sleep(for: .milliseconds(700))
            self?.setStatus(of: m.id, in: chat, .delivered)
            try? await Task.sleep(for: .milliseconds(1200))
            self?.setStatus(of: m.id, in: chat, .read)
            guard let self, let replier = self.replier(for: chat) else { return }
            self.emit(.typing(chat: chat, user: replier, active: true))
            try? await Task.sleep(for: .milliseconds(1800))
            self.emit(.typing(chat: chat, user: replier, active: false))
            let reply = self.locked { () -> ChatMessage in
                let r = ChatMessage(id: Self.randomID(), chatID: chat, sender: replier, isOutgoing: false,
                                    body: Self.replies.randomElement() ?? "👍", sentAt: Date(), status: .received)
                self.append(r)
                if var c = self.chatsByID[chat] { c.unread += 1; self.chatsByID[chat] = c }
                return r
            }
            self.emit(.messageAdded(chat: chat, message: reply.id))
            self.emit(.notify(chat: chat, message: reply.id, sender: replier))
            self.emit(.chatsChanged)
        }
    }

    private static let replies = [
        "Haha, true 😄", "Sounds good!", "On my way", "Love that", "Tell me more", "👍", "Can't wait",
        "That's wild", "Okay, deal", "Let me check and get back to you",
    ]

    private func replier(for chat: ChatID) -> UserID? {
        locked { (membersByChat[chat] ?? []).filter { $0 != Self.collins }.randomElement() }
    }

    private func mutate(_ id: MessageID, in chat: ChatID, _ change: (inout ChatMessage) throws -> Void) throws {
        try locked {
            guard let i = messagesByChat[chat]?.firstIndex(where: { $0.id == id }) else { throw notFound() }
            try change(&messagesByChat[chat]![i])
        }
        emit(.messageUpdated(chat: chat, message: id))
    }

    public func react(_ emoji: String, to message: MessageID, in chat: ChatID, remove: Bool) throws {
        try mutate(message, in: chat) { m in
            if let i = m.reactions.firstIndex(where: { $0.emoji == emoji }) {
                if remove, m.reactions[i].mine {
                    m.reactions[i].count -= 1
                    m.reactions[i].mine = false
                    if m.reactions[i].count <= 0 { m.reactions.remove(at: i) }
                } else if !remove, !m.reactions[i].mine {
                    m.reactions[i].count += 1
                    m.reactions[i].mine = true
                }
            } else if !remove {
                m.reactions.append(Reaction(emoji: emoji, count: 1, mine: true))
            }
        }
    }

    public func edit(_ message: MessageID, in chat: ChatID, text: String) throws {
        try mutate(message, in: chat) { m in
            guard m.isOutgoing, m.kind == .text else { throw CoreFailure(.notAllowed, "You can only edit your own messages") }
            m.body = text
            m.isEdited = true
        }
        emit(.chatsChanged)
    }

    public func deleteForEveryone(_ message: MessageID, in chat: ChatID) throws {
        try mutate(message, in: chat) { m in
            guard m.isOutgoing else { throw CoreFailure(.notAllowed, "You can only delete your own messages for everyone") }
            m.isDeleted = true
            m.body = ""
            m.attachment = nil
            m.reactions = []
        }
        emit(.chatsChanged)
    }

    public func deleteForMe(_ message: MessageID, in chat: ChatID) throws {
        locked { messagesByChat[chat]?.removeAll { $0.id == message } }
        emit(.messageRemoved(chat: chat, message: message))
        emit(.chatsChanged)
    }

    public func setTyping(_ active: Bool, in chat: ChatID) throws {}

    public func markRead(_ chat: ChatID) throws {
        locked {
            chatsByID[chat]?.unread = 0
            for i in (messagesByChat[chat] ?? []).indices where messagesByChat[chat]?[i].status == .received {
                messagesByChat[chat]?[i].status = .seen
            }
        }
        emit(.chatsChanged)
    }

    public func setFlag(_ flag: ChatFlag, _ value: Bool, for chat: ChatID) throws {
        try locked {
            guard chatsByID[chat] != nil else { throw notFound() }
            switch flag {
            case .muted: chatsByID[chat]?.isMuted = value
            case .pinned: chatsByID[chat]?.isPinned = value
            case .archived: chatsByID[chat]?.isArchived = value
            }
        }
        emit(.chatsChanged)
        emit(.chatUpdated(chat))
    }

    public func setDisappearingTimer(_ seconds: UInt32, for chat: ChatID) throws {
        let line = seconds == 0 ? "You turned off disappearing messages"
            : "You set disappearing messages to \(DisappearingTimer.label(seconds).lowercased())"
        locked {
            chatsByID[chat]?.timerSeconds = seconds
            system(chat, line, Date())
        }
        emit(.chatUpdated(chat))
        emit(.chatsChanged)
    }

    public func updateGroup(_ chat: ChatID, name: String, description: String, avatar: Data?) throws {
        try locked {
            guard let c = chatsByID[chat] else { throw notFound() }
            guard c.isAdmin else { throw CoreFailure(.notAllowed, "Only admins can change the group") }
            chatsByID[chat]?.title = name
            chatsByID[chat]?.description = description
            if let avatar { chatsByID[chat]?.avatar = avatar }
        }
        emit(.chatUpdated(chat))
        emit(.chatsChanged)
    }

    public func deleteChatLocally(_ chat: ChatID) throws {
        locked {
            chatsByID[chat] = nil
            messagesByChat[chat] = nil
        }
        emit(.chatsChanged)
    }

    public func setBlocked(_ user: UserID, _ blocked: Bool) throws {
        locked { contactsByID[user]?.isBlocked = blocked }
        emit(.contactsChanged)
    }

    public func retry(_ message: MessageID) async throws {
        let m = try self.message(message)
        setStatus(of: message, in: m.chatID, .pending)
        if simulate { progress(m) }
    }

    // MARK: Groups

    public func createChat(kind: ChatKind, name: String, members: [UserID]) async throws -> ChatID {
        if kind == .direct, let peer = members.first,
           let existing = locked({ chatsByID.values.first { $0.kind == .direct && $0.peer == peer } }) {
            return existing.id
        }
        let id = Self.randomID(32)
        locked {
            let title = kind == .direct ? (members.first.flatMap { contactsByID[$0]?.name } ?? "Friend") : name
            addChat(id, kind: kind, title: title, peer: kind == .direct ? members.first : nil,
                    members: [Self.collins] + members, admins: [Self.collins])
            system(id, kind == .direct ? "Messages in this chat are end-to-end encrypted. Only the two of you can read them." : "You created “\(name)”", Date())
        }
        emit(.chatsChanged)
        return id
    }

    public func addMembers(_ users: [UserID], to chat: ChatID) async throws {
        locked {
            membersByChat[chat, default: []].append(contentsOf: users)
            chatsByID[chat]?.memberCount = membersByChat[chat]?.count ?? 0
            let names = users.compactMap { contactsByID[$0]?.name }.joined(separator: ", ")
            system(chat, "You added \(names)", Date())
        }
        emit(.chatUpdated(chat))
    }

    public func removeMember(_ user: UserID, from chat: ChatID) async throws {
        locked {
            membersByChat[chat]?.removeAll { $0 == user }
            chatsByID[chat]?.memberCount = membersByChat[chat]?.count ?? 0
            system(chat, "You removed \(contactsByID[user]?.name ?? "someone")", Date())
        }
        emit(.chatUpdated(chat))
    }

    public func setAdmin(_ user: UserID, _ admin: Bool, in chat: ChatID) async throws {
        locked {
            if admin { adminsByChat[chat, default: []].insert(user) } else { adminsByChat[chat]?.remove(user) }
        }
        emit(.chatUpdated(chat))
    }

    public func leave(_ chat: ChatID) async throws {
        locked {
            chatsByID[chat]?.hasLeft = true
            system(chat, "You left", Date())
        }
        emit(.chatsChanged)
        emit(.chatUpdated(chat))
    }

    // MARK: Pairing and verification

    public func classify(code: String) throws -> (kind: CodeKind, name: String) {
        guard code.hasPrefix("ASIRI1:") else { throw CoreFailure(.pairing, "This isn't an Asiri code") }
        if code.contains("offer") { return (.deviceOffer, "Dami") }
        if code.contains("card") { return (.contact, "Funmi") }
        return (.invitation, "Collins")
    }

    public func deviceOfferCode() throws -> String { "ASIRI1:offer-preview-0123456789abcdefghijklmnopqrstuvwxyz" }
    public func contactCode() throws -> String { "ASIRI1:card-preview-0123456789abcdefghijklmnopqrstuvwxyz" }

    private static let sixEmoji = [
        EmojiWord(glyph: "🦊", name: "Fox"), EmojiWord(glyph: "🌵", name: "Cactus"), EmojiWord(glyph: "🎈", name: "Balloon"),
        EmojiWord(glyph: "🐢", name: "Turtle"), EmojiWord(glyph: "🍋", name: "Lemon"), EmojiWord(glyph: "🚀", name: "Rocket"),
    ]

    public func inviteDevice(offer: String) async throws -> PairingResult {
        let (kind, name) = try classify(code: offer)
        guard kind == .deviceOffer else { throw CoreFailure(.pairing, "That's not a new device's code") }
        let peer = Self.randomID()
        locked { contactsByID[peer] = Contact(id: peer, name: name) }
        if simulate {
            Task.detached { [weak self] in
                try? await Task.sleep(for: .seconds(2))
                guard let self else { return }
                let chat = try? await self.createChat(kind: .direct, name: "", members: [peer])
                if let chat { self.emit(.pairingCompleted(chat: chat, user: peer)) }
            }
        }
        return PairingResult(emoji: Self.sixEmoji, peerName: name, peerID: peer, replyCode: "ASIRI1:invite-preview-\(peer.base64EncodedString())")
    }

    public func redeemInvitation(_ code: String) async throws -> PairingResult {
        _ = try classify(code: code)
        try? await Task.sleep(for: .milliseconds(simulate ? 900 : 0))
        locked {
            me.isRegistered = true
            me.relayAddress = "ksfz7l2…onion"
            me.deviceID = Self.randomID()
            if code.contains("admin") { me.isAdmin = true }
        }
        setStatus(.connected)
        if code.contains("admin") {
            return PairingResult(emoji: [], peerName: "", peerID: Data(count: 16))
        }
        let chat = try await createChat(kind: .direct, name: "", members: [Self.ada])
        return PairingResult(emoji: Self.sixEmoji, peerName: "Ada Okafor", peerID: Self.ada, chatID: chat)
    }

    public func addContact(card: String) async throws -> PairingResult {
        let (_, name) = try classify(code: card)
        let peer = Self.randomID()
        locked { contactsByID[peer] = Contact(id: peer, name: name) }
        let chat = try await createChat(kind: .direct, name: "", members: [peer])
        return PairingResult(emoji: Self.sixEmoji, peerName: name, peerID: peer, chatID: chat)
    }

    public func confirmVerification(of user: UserID, inPerson: Bool) throws {
        locked {
            contactsByID[user]?.verification = inPerson ? .inPerson : .remote
            for (id, c) in chatsByID where c.peer == user {
                chatsByID[id]?.verification = inPerson ? .inPerson : .remote
            }
        }
        emit(.contactsChanged)
        emit(.chatsChanged)
    }

    public func pairingEmoji(for user: UserID) throws -> [EmojiWord] { Self.sixEmoji }

    public func safetyNumber(for user: UserID) throws -> String {
        "37281 90412 55803 11698 40275 66031\n82946 10357 29018 74652 03981 56720"
    }

    // MARK: Files

    public func sendFile(_ file: OutgoingFile, caption: String, in chat: ChatID, replyTo: MessageID?) async throws -> MessageID {
        let attachment = Attachment(size: UInt64(file.data.count), mime: file.mime, fileName: file.fileName, width: file.width,
                                    height: file.height, durationMs: file.durationMs, thumbnail: file.thumbnail,
                                    blurhash: file.blurhash, isVoiceNote: file.isVoiceNote, waveform: file.waveform,
                                    state: .uploading, progress: 0)
        let m = ChatMessage(id: Self.randomID(), chatID: chat, sender: Self.collins, isOutgoing: true, kind: .attachment,
                            body: caption, replyTo: replyTo, sentAt: Date(), status: .pending, attachment: attachment)
        locked { append(m) }
        emit(.messageAdded(chat: chat, message: m.id))
        emit(.chatsChanged)
        if simulate {
            Task.detached { [weak self] in
                for step in 1 ... 5 {
                    try? await Task.sleep(for: .milliseconds(180))
                    try? self?.mutate(m.id, in: chat) { $0.attachment?.progress = Double(step) / 5 }
                }
                try? self?.mutate(m.id, in: chat) { $0.attachment?.state = .local }
                self?.progress(m)
            }
        }
        return m.id
    }

    public func download(_ message: MessageID) async throws {
        let m = try self.message(message)
        try mutate(message, in: m.chatID) { $0.attachment?.state = .local; $0.attachment?.progress = 1 }
    }

    public func attachmentData(_ message: MessageID) throws -> Data { Data() }
    public func exportAttachment(_ message: MessageID, to url: URL) throws {}

    // MARK: Calls

    public func startCall(in chat: ChatID, video: Bool) async throws -> CallSession {
        let id = Self.randomID()
        locked { calls[id] = CallRecord(id: id, chatID: chat, isVideo: video, isOutgoing: true, caller: Self.collins, phase: .ringing, startedAt: Date(), isJoined: true) }
        return CallSession(callID: id, chatID: chat, url: "ws://127.0.0.1:0", token: "", keyBase64: "", keyIndex: 0,
                           emoji: Array(Self.sixEmoji.prefix(4)), isVideo: video)
    }

    public func acceptCall(_ call: CallID) async throws -> CallSession {
        let record = try locked { () throws -> CallRecord in
            guard var c = calls[call] else { throw notFound() }
            c.phase = .active
            c.answeredAt = Date()
            c.isJoined = true
            calls[call] = c
            return c
        }
        emit(.callUpdated(call))
        return CallSession(callID: call, chatID: record.chatID, url: "ws://127.0.0.1:0", token: "", keyBase64: "", keyIndex: 0,
                           emoji: Array(Self.sixEmoji.prefix(4)), isVideo: record.isVideo)
    }

    public func declineCall(_ call: CallID) throws { endCall(call) }

    public func endCall(_ call: CallID) {
        locked {
            calls[call]?.phase = .ended
            calls[call]?.endedAt = Date()
            calls[call]?.isJoined = false
        }
        emit(.callUpdated(call))
    }

    public func call(_ id: CallID) throws -> CallRecord {
        try locked {
            guard let c = calls[id] else { throw notFound() }
            return c
        }
    }

    public func recentCalls(limit: Int) throws -> [CallRecord] {
        locked { Array(calls.values.sorted { $0.startedAt > $1.startedAt }.prefix(limit)) }
    }

    public func currentCallKey(_ call: CallID) throws -> (keyBase64: String, keyIndex: UInt32) { ("", 0) }

    public func callParticipant(_ call: CallID, alias: String) throws -> UserID? { nil }

    /// Rings this Mac, as if a friend were calling (previews and the demo).
    public func simulateIncomingCall(from user: UserID, in chat: ChatID, video: Bool) {
        let id = Self.randomID()
        locked { calls[id] = CallRecord(id: id, chatID: chat, isVideo: video, isOutgoing: false, caller: user, phase: .ringing, startedAt: Date()) }
        emit(.incomingCall(IncomingCall(callID: id, chatID: chat, caller: user, isVideo: video)))
    }

    // MARK: Server, updates, wipe

    public func serverStatus() async throws -> ServerStatus {
        ServerStatus(devices: [
            ServerDevice(id: Self.id(0xD0), state: .active, isAdmin: true),
            ServerDevice(id: Self.id(0xD1), state: .active, isAdmin: false),
            ServerDevice(id: Self.id(0xD2), state: .active, isAdmin: false),
            ServerDevice(id: Self.id(0xD3), state: .pending, isAdmin: false),
        ], invitePolicy: "admins", storedBytes: 182_000_000, queuedEnvelopes: 12)
    }

    public func revokeDevice(_ device: DeviceID) async throws {}
    public func checkForUpdate() async throws -> UpdateInfo? { nil }
    public func downloadUpdate() async throws -> URL { throw CoreFailure(.notFound, "No update") }

    public func panicWipe() async throws {
        locked {
            chatsByID = [:]
            messagesByChat = [:]
            contactsByID = [:]
            me.isRegistered = false
        }
        setStatus(.notRegistered)
    }

    // MARK: Lookup helpers for previews

    /// The demo contact with this ID.
    public func contact(_ id: UserID) -> Contact? { locked { contactsByID[id] } }
}
