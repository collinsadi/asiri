import Foundation

// The app's view of the world. Everything here is built from the encrypted database by the
// core; nothing is fetched to build it (architecture §13.2). IDs are opaque bytes: 16 for users,
// messages, devices and calls, 32 for chats.

public typealias UserID = Data
public typealias ChatID = Data
public typealias MessageID = Data
public typealias CallID = Data
public typealias DeviceID = Data

public enum ChatKind: Sendable, Hashable {
    case direct
    case group
}

public enum Verification: Sendable, Hashable {
    /// Never verified.
    case unverified
    /// QR codes scanned in person and emoji confirmed.
    case inPerson
    /// Safety number compared remotely.
    case remote
    /// Their key changed: verify again before trusting the chat.
    case changed

    public var isVerified: Bool { self == .inPerson || self == .remote }
}

public enum MessageKind: Sendable, Hashable {
    case text
    case attachment
    case system
    case call
    case unsupported
}

/// Delivery state (architecture §13.5).
public enum DeliveryStatus: Sendable, Hashable {
    /// Saved on this Mac, waiting to send (clock).
    case pending
    /// Accepted by the server (one tick).
    case sent
    /// On every recipient's Mac (two ticks).
    case delivered
    /// Read (two accent ticks).
    case read
    /// Couldn't send (red exclamation mark).
    case failed
    /// Someone else's message.
    case received
    /// Someone else's message the user has read.
    case seen
}

public enum ConnectionStatus: Sendable, Hashable {
    case notRegistered
    case offline
    case startingTor
    case connecting
    case syncing
    case connected

    /// Plain-language status for the title bar and menu bar.
    public var label: String {
        switch self {
        case .notRegistered: "Not connected to a server"
        case .offline: "Waiting for network…"
        case .startingTor: "Connecting securely…"
        case .connecting: "Connecting…"
        case .syncing: "Updating…"
        case .connected: "Connected"
        }
    }

    public var isOnline: Bool { self == .connected || self == .syncing }
}

public enum TransferState: Sendable, Hashable {
    case remote
    case downloading
    case local
    case failed
    case uploading
}

public struct Attachment: Sendable, Hashable {
    public var size: UInt64
    public var mime: String
    public var fileName: String
    public var width: Int
    public var height: Int
    public var durationMs: Int
    public var thumbnail: Data?
    public var blurhash: String
    public var isVoiceNote: Bool
    public var waveform: [UInt8]
    public var state: TransferState
    public var progress: Double

    public init(
        size: UInt64, mime: String, fileName: String, width: Int = 0, height: Int = 0,
        durationMs: Int = 0, thumbnail: Data? = nil, blurhash: String = "", isVoiceNote: Bool = false,
        waveform: [UInt8] = [], state: TransferState = .remote, progress: Double = 0
    ) {
        self.size = size
        self.mime = mime
        self.fileName = fileName
        self.width = width
        self.height = height
        self.durationMs = durationMs
        self.thumbnail = thumbnail
        self.blurhash = blurhash
        self.isVoiceNote = isVoiceNote
        self.waveform = waveform
        self.state = state
        self.progress = progress
    }

    public var isImage: Bool { mime.hasPrefix("image/") }
    public var isVideo: Bool { mime.hasPrefix("video/") }
    public var aspectRatio: CGFloat {
        guard width > 0, height > 0 else { return 4.0 / 3.0 }
        return CGFloat(width) / CGFloat(height)
    }
}

public struct Reaction: Sendable, Hashable {
    public var emoji: String
    public var count: Int
    public var mine: Bool

    public init(emoji: String, count: Int, mine: Bool) {
        self.emoji = emoji
        self.count = count
        self.mine = mine
    }
}

public struct ChatMessage: Identifiable, Sendable, Hashable {
    public var id: MessageID
    public var chatID: ChatID
    public var sender: UserID
    public var isOutgoing: Bool
    public var kind: MessageKind
    public var body: String
    public var replyTo: MessageID?
    public var sentAt: Date
    public var receivedAt: Date
    public var sortKey: Int64
    public var status: DeliveryStatus
    public var isEdited: Bool
    public var isDeleted: Bool
    public var expiresAt: Date?
    public var attachment: Attachment?
    public var reactions: [Reaction]

    public init(
        id: MessageID, chatID: ChatID, sender: UserID, isOutgoing: Bool, kind: MessageKind = .text,
        body: String, replyTo: MessageID? = nil, sentAt: Date, receivedAt: Date? = nil,
        sortKey: Int64? = nil, status: DeliveryStatus, isEdited: Bool = false, isDeleted: Bool = false,
        expiresAt: Date? = nil, attachment: Attachment? = nil, reactions: [Reaction] = []
    ) {
        self.id = id
        self.chatID = chatID
        self.sender = sender
        self.isOutgoing = isOutgoing
        self.kind = kind
        self.body = body
        self.replyTo = replyTo
        self.sentAt = sentAt
        self.receivedAt = receivedAt ?? sentAt
        self.sortKey = sortKey ?? Int64(sentAt.timeIntervalSince1970 * 1000)
        self.status = status
        self.isEdited = isEdited
        self.isDeleted = isDeleted
        self.expiresAt = expiresAt
        self.attachment = attachment
        self.reactions = reactions
    }

    /// Whether this is a bubble (as opposed to a centered system or call line).
    public var isBubble: Bool { kind == .text || kind == .attachment || kind == .unsupported }

    /// A one-line preview for the chat list and reply quotes.
    public var preview: String {
        if isDeleted { return "Deleted message" }
        switch kind {
        case .text, .system, .call, .unsupported:
            return body
        case .attachment:
            guard let a = attachment else { return body }
            let label = a.isVoiceNote ? "Voice message" : a.isImage ? "Photo" : a.isVideo ? "Video" : a.fileName
            return body.isEmpty ? label : "\(label) · \(body)"
        }
    }
}

public struct Chat: Identifiable, Sendable, Hashable {
    public var id: ChatID
    public var kind: ChatKind
    public var title: String
    public var description: String
    public var avatar: Data?
    public var peer: UserID?
    public var lastMessage: ChatMessage?
    public var lastSenderName: String?
    public var verification: Verification?
    public var memberCount: Int
    public var isAdmin: Bool
    public var unread: Int
    public var isMuted: Bool
    public var isPinned: Bool
    public var isArchived: Bool
    public var timerSeconds: UInt32
    public var hasLeft: Bool
    public var lastActivity: Date

    public init(
        id: ChatID, kind: ChatKind, title: String, description: String = "", avatar: Data? = nil,
        peer: UserID? = nil, lastMessage: ChatMessage? = nil, lastSenderName: String? = nil,
        verification: Verification? = nil, memberCount: Int = 2, isAdmin: Bool = false, unread: Int = 0,
        isMuted: Bool = false, isPinned: Bool = false, isArchived: Bool = false, timerSeconds: UInt32 = 0,
        hasLeft: Bool = false, lastActivity: Date
    ) {
        self.id = id
        self.kind = kind
        self.title = title
        self.description = description
        self.avatar = avatar
        self.peer = peer
        self.lastMessage = lastMessage
        self.lastSenderName = lastSenderName
        self.verification = verification
        self.memberCount = memberCount
        self.isAdmin = isAdmin
        self.unread = unread
        self.isMuted = isMuted
        self.isPinned = isPinned
        self.isArchived = isArchived
        self.timerSeconds = timerSeconds
        self.hasLeft = hasLeft
        self.lastActivity = lastActivity
    }
}

public struct Contact: Identifiable, Sendable, Hashable {
    public var id: UserID
    public var name: String
    public var avatar: Data?
    public var verification: Verification
    public var isBlocked: Bool

    public init(id: UserID, name: String, avatar: Data? = nil, verification: Verification = .unverified, isBlocked: Bool = false) {
        self.id = id
        self.name = name
        self.avatar = avatar
        self.verification = verification
        self.isBlocked = isBlocked
    }

    public var displayName: String { name.isEmpty ? "Friend" : name }
}

public struct Member: Identifiable, Sendable, Hashable {
    public var contact: Contact
    public var isAdmin: Bool
    public var isMe: Bool
    public var id: UserID { contact.id }

    public init(contact: Contact, isAdmin: Bool, isMe: Bool) {
        self.contact = contact
        self.isAdmin = isAdmin
        self.isMe = isMe
    }
}

public struct Profile: Sendable, Hashable {
    public var userID: UserID
    public var name: String
    public var avatar: Data?
    public var fingerprint: String
    public var isRegistered: Bool
    public var isAdmin: Bool
    public var relayAddress: String?
    public var isDirectTransport: Bool
    public var onionClientKey: String
    public var deviceID: DeviceID?

    public init(
        userID: UserID, name: String, avatar: Data? = nil, fingerprint: String, isRegistered: Bool,
        isAdmin: Bool, relayAddress: String? = nil, isDirectTransport: Bool = false,
        onionClientKey: String = "", deviceID: DeviceID? = nil
    ) {
        self.userID = userID
        self.name = name
        self.avatar = avatar
        self.fingerprint = fingerprint
        self.isRegistered = isRegistered
        self.isAdmin = isAdmin
        self.relayAddress = relayAddress
        self.isDirectTransport = isDirectTransport
        self.onionClientKey = onionClientKey
        self.deviceID = deviceID
    }
}

public struct EmojiWord: Sendable, Hashable {
    public var glyph: String
    public var name: String

    public init(glyph: String, name: String) {
        self.glyph = glyph
        self.name = name
    }
}

public struct PairingResult: Sendable, Hashable {
    public var emoji: [EmojiWord]
    public var peerName: String
    public var peerID: UserID
    public var replyCode: String?
    public var chatID: ChatID?

    public init(emoji: [EmojiWord], peerName: String, peerID: UserID, replyCode: String? = nil, chatID: ChatID? = nil) {
        self.emoji = emoji
        self.peerName = peerName
        self.peerID = peerID
        self.replyCode = replyCode
        self.chatID = chatID
    }
}

public enum CodeKind: Sendable, Hashable {
    /// A new Mac asking to be invited (QR-A).
    case deviceOffer
    /// An invitation to join a server.
    case invitation
    /// A friend who already uses Asiri.
    case contact
}

public struct CallSession: Sendable, Hashable {
    public var callID: CallID
    public var chatID: ChatID
    public var url: String
    public var token: String
    public var keyBase64: String
    public var keyIndex: UInt32
    public var emoji: [EmojiWord]
    public var isVideo: Bool

    public init(callID: CallID, chatID: ChatID, url: String, token: String, keyBase64: String, keyIndex: UInt32, emoji: [EmojiWord], isVideo: Bool) {
        self.callID = callID
        self.chatID = chatID
        self.url = url
        self.token = token
        self.keyBase64 = keyBase64
        self.keyIndex = keyIndex
        self.emoji = emoji
        self.isVideo = isVideo
    }
}

public enum CallPhase: Sendable, Hashable {
    case ringing
    case active
    case ended
}

public struct CallRecord: Identifiable, Sendable, Hashable {
    public var id: CallID
    public var chatID: ChatID
    public var isVideo: Bool
    public var isOutgoing: Bool
    public var caller: UserID
    public var phase: CallPhase
    public var startedAt: Date
    public var answeredAt: Date?
    public var endedAt: Date?
    public var isJoined: Bool

    public init(id: CallID, chatID: ChatID, isVideo: Bool, isOutgoing: Bool, caller: UserID, phase: CallPhase, startedAt: Date, answeredAt: Date? = nil, endedAt: Date? = nil, isJoined: Bool = false) {
        self.id = id
        self.chatID = chatID
        self.isVideo = isVideo
        self.isOutgoing = isOutgoing
        self.caller = caller
        self.phase = phase
        self.startedAt = startedAt
        self.answeredAt = answeredAt
        self.endedAt = endedAt
        self.isJoined = isJoined
    }
}

public struct IncomingCall: Sendable, Hashable, Identifiable {
    public var callID: CallID
    public var chatID: ChatID
    public var caller: UserID
    public var isVideo: Bool
    public var id: CallID { callID }

    public init(callID: CallID, chatID: ChatID, caller: UserID, isVideo: Bool) {
        self.callID = callID
        self.chatID = chatID
        self.caller = caller
        self.isVideo = isVideo
    }
}

public struct AppSettings: Sendable, Hashable {
    public var readReceipts: Bool
    public var typingIndicators: Bool
    public var linkPreviews: Bool
    public var defaultTimerSeconds: UInt32
    public var autoDownloadImages: Bool
    public var autoDownloadMaxBytes: UInt64

    public init(readReceipts: Bool = true, typingIndicators: Bool = true, linkPreviews: Bool = false, defaultTimerSeconds: UInt32 = 0, autoDownloadImages: Bool = true, autoDownloadMaxBytes: UInt64 = 25 * 1024 * 1024) {
        self.readReceipts = readReceipts
        self.typingIndicators = typingIndicators
        self.linkPreviews = linkPreviews
        self.defaultTimerSeconds = defaultTimerSeconds
        self.autoDownloadImages = autoDownloadImages
        self.autoDownloadMaxBytes = autoDownloadMaxBytes
    }
}

public struct SearchHit: Identifiable, Sendable, Hashable {
    public var chatID: ChatID
    public var messageID: MessageID
    public var snippet: String
    public var sentAt: Date
    public var id: MessageID { messageID }

    public init(chatID: ChatID, messageID: MessageID, snippet: String, sentAt: Date) {
        self.chatID = chatID
        self.messageID = messageID
        self.snippet = snippet
        self.sentAt = sentAt
    }
}

public struct ServerDevice: Identifiable, Sendable, Hashable {
    public enum State: Sendable, Hashable { case pending, active, revoked }
    public var id: DeviceID
    public var state: State
    public var isAdmin: Bool

    public init(id: DeviceID, state: State, isAdmin: Bool) {
        self.id = id
        self.state = state
        self.isAdmin = isAdmin
    }
}

public struct ServerStatus: Sendable, Hashable {
    public var devices: [ServerDevice]
    public var invitePolicy: String
    public var storedBytes: UInt64
    public var queuedEnvelopes: UInt64

    public init(devices: [ServerDevice], invitePolicy: String, storedBytes: UInt64, queuedEnvelopes: UInt64) {
        self.devices = devices
        self.invitePolicy = invitePolicy
        self.storedBytes = storedBytes
        self.queuedEnvelopes = queuedEnvelopes
    }
}

public struct UpdateInfo: Sendable, Hashable {
    public var version: String
    public var build: UInt32
    public var notes: String
    public var size: UInt64
    public var isRequired: Bool

    public init(version: String, build: UInt32, notes: String, size: UInt64, isRequired: Bool) {
        self.version = version
        self.build = build
        self.notes = notes
        self.size = size
        self.isRequired = isRequired
    }
}

/// A file prepared for sending (metadata already stripped).
public struct OutgoingFile: Sendable, Hashable {
    public var data: Data
    public var mime: String
    public var fileName: String
    public var width: Int
    public var height: Int
    public var durationMs: Int
    public var thumbnail: Data?
    public var blurhash: String
    public var isVoiceNote: Bool
    public var waveform: [UInt8]

    public init(data: Data, mime: String, fileName: String, width: Int = 0, height: Int = 0, durationMs: Int = 0, thumbnail: Data? = nil, blurhash: String = "", isVoiceNote: Bool = false, waveform: [UInt8] = []) {
        self.data = data
        self.mime = mime
        self.fileName = fileName
        self.width = width
        self.height = height
        self.durationMs = durationMs
        self.thumbnail = thumbnail
        self.blurhash = blurhash
        self.isVoiceNote = isVoiceNote
        self.waveform = waveform
    }
}

public enum ChatFlag: Sendable, Hashable {
    case muted
    case pinned
    case archived
}

/// Something the UI reacts to.
public enum CoreEvent: Sendable, Hashable {
    case connection(ConnectionStatus)
    case chatsChanged
    case chatUpdated(ChatID)
    case messageAdded(chat: ChatID, message: MessageID)
    case messageUpdated(chat: ChatID, message: MessageID)
    case messageRemoved(chat: ChatID, message: MessageID)
    case typing(chat: ChatID, user: UserID, active: Bool)
    case notify(chat: ChatID, message: MessageID, sender: UserID)
    case contactsChanged
    case pairingCompleted(chat: ChatID, user: UserID)
    case incomingCall(IncomingCall)
    case callUpdated(CallID)
    case callKeyRotated(call: CallID, keyBase64: String, keyIndex: UInt32)
    case securityWarning(chat: ChatID?, text: String)
    case updateAvailable(version: String, notes: String, isRequired: Bool)
}

/// Why something failed, in words the user can read.
public struct CoreFailure: Error, LocalizedError, Sendable, Hashable {
    public enum Kind: Sendable, Hashable {
        case offline, server, pairing, invalid, notFound, notRegistered, notAllowed, crypto, storage, network, `internal`
    }

    public var kind: Kind
    public var message: String

    public init(_ kind: Kind, _ message: String) {
        self.kind = kind
        self.message = message
    }

    public var errorDescription: String? { message }
}
