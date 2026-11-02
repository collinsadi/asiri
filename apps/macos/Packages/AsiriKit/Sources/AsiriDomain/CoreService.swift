import Foundation

/// Everything the screens need from the engine. `LiveCore` (AsiriCoreBridge) implements it with
/// the Rust core; `PreviewCore` implements it in memory for previews, snapshots and tests.
///
/// Synchronous calls only touch the local database and return quickly. `async` calls go to the
/// network. All calls are safe from any thread.
public protocol CoreService: AnyObject, Sendable {
    /// Events, for the one consumer that owns the app state.
    var events: AsyncStream<CoreEvent> { get }

    // MARK: Profile, settings, connection
    func profile() throws -> Profile
    func setProfile(name: String, avatar: Data?) throws
    func settings() throws -> AppSettings
    func setSettings(_ settings: AppSettings) throws
    func connectionStatus() -> ConnectionStatus
    func connect() throws
    func notifyNetworkChange()
    func notifyWake(asleepSeconds: UInt64) throws

    // MARK: Chats and messages
    func chats() throws -> [Chat]
    func chat(_ id: ChatID) throws -> Chat
    func messages(in chat: ChatID, before: MessageID?, limit: Int) throws -> [ChatMessage]
    func message(_ id: MessageID) throws -> ChatMessage
    func members(of chat: ChatID) throws -> [Member]
    func contacts() throws -> [Contact]
    func search(_ query: String, limit: Int) throws -> [SearchHit]
    func sendText(_ text: String, in chat: ChatID, replyTo: MessageID?) throws -> MessageID
    func react(_ emoji: String, to message: MessageID, in chat: ChatID, remove: Bool) throws
    func edit(_ message: MessageID, in chat: ChatID, text: String) throws
    func deleteForEveryone(_ message: MessageID, in chat: ChatID) throws
    func deleteForMe(_ message: MessageID, in chat: ChatID) throws
    func setTyping(_ active: Bool, in chat: ChatID) throws
    func markRead(_ chat: ChatID) throws
    func setFlag(_ flag: ChatFlag, _ value: Bool, for chat: ChatID) throws
    func setDisappearingTimer(_ seconds: UInt32, for chat: ChatID) throws
    func updateGroup(_ chat: ChatID, name: String, description: String, avatar: Data?) throws
    func deleteChatLocally(_ chat: ChatID) throws
    func setBlocked(_ user: UserID, _ blocked: Bool) throws
    func retry(_ message: MessageID) async throws

    // MARK: Groups
    func createChat(kind: ChatKind, name: String, members: [UserID]) async throws -> ChatID
    func addMembers(_ users: [UserID], to chat: ChatID) async throws
    func removeMember(_ user: UserID, from chat: ChatID) async throws
    func setAdmin(_ user: UserID, _ admin: Bool, in chat: ChatID) async throws
    func leave(_ chat: ChatID) async throws

    // MARK: Pairing and verification
    func classify(code: String) throws -> (kind: CodeKind, name: String)
    func deviceOfferCode() throws -> String
    func contactCode() throws -> String
    func inviteDevice(offer: String) async throws -> PairingResult
    func redeemInvitation(_ code: String) async throws -> PairingResult
    func addContact(card: String) async throws -> PairingResult
    func confirmVerification(of user: UserID, inPerson: Bool) throws
    func pairingEmoji(for user: UserID) throws -> [EmojiWord]
    func safetyNumber(for user: UserID) throws -> String

    // MARK: Files
    func sendFile(_ file: OutgoingFile, caption: String, in chat: ChatID, replyTo: MessageID?) async throws -> MessageID
    func download(_ message: MessageID) async throws
    func attachmentData(_ message: MessageID) throws -> Data
    func exportAttachment(_ message: MessageID, to url: URL) throws

    // MARK: Calls
    func startCall(in chat: ChatID, video: Bool) async throws -> CallSession
    func acceptCall(_ call: CallID) async throws -> CallSession
    func declineCall(_ call: CallID) throws
    func endCall(_ call: CallID) throws
    func call(_ id: CallID) throws -> CallRecord
    func recentCalls(limit: Int) throws -> [CallRecord]
    func currentCallKey(_ call: CallID) throws -> (keyBase64: String, keyIndex: UInt32)
    /// The person behind a LiveKit identity (a random alias) in a call, once a call signal has
    /// said; `nil` until then.
    func callParticipant(_ call: CallID, alias: String) throws -> UserID?

    // MARK: Server, updates, wipe
    func serverStatus() async throws -> ServerStatus
    func revokeDevice(_ device: DeviceID) async throws
    func checkForUpdate() async throws -> UpdateInfo?
    func downloadUpdate() async throws -> URL
    func panicWipe() async throws
}

/// Timer choices offered in the UI.
public enum DisappearingTimer {
    public static let choices: [UInt32] = [0, 30, 300, 3_600, 28_800, 86_400, 604_800, 2_419_200]

    public static func label(_ seconds: UInt32) -> String {
        switch seconds {
        case 0: "Off"
        case 30: "30 seconds"
        case 300: "5 minutes"
        case 3_600: "1 hour"
        case 28_800: "8 hours"
        case 86_400: "1 day"
        case 604_800: "1 week"
        case 2_419_200: "4 weeks"
        default: "\(seconds) seconds"
        }
    }
}
