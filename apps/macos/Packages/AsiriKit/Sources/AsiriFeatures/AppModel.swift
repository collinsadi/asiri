import AppKit
import AsiriDomain
import AsiriPlatform
import Foundation
import Observation

/// Chat list filters (§13.3).
public enum ChatFilter: String, CaseIterable, Identifiable, Sendable {
    case all = "All"
    case unread = "Unread"
    case groups = "Groups"
    case archived = "Archived"

    public var id: String { rawValue }
}

/// A short message that slides in at the top of the window.
public struct Toast: Identifiable, Equatable {
    public enum Style: Equatable { case info, success, warning, error }
    public let id = UUID()
    public var style: Style
    public var text: String

    public init(_ style: Style, _ text: String) {
        self.style = style
        self.text = text
    }
}

/// Sheets the main window can show.
public enum AppSheet: Identifiable, Equatable {
    case newChat
    case newGroup
    case addFriend
    case addMembers(ChatID)
    case verify(UserID)

    public var id: String {
        switch self {
        case .newChat: "newChat"
        case .newGroup: "newGroup"
        case .addFriend: "addFriend"
        case let .addMembers(c): "addMembers-\(c.base64EncodedString())"
        case let .verify(u): "verify-\(u.base64EncodedString())"
        }
    }
}

/// The app's state, owned by the main actor (architecture §6.4). It consumes the core's events
/// and keeps the chat list, the open conversations and everything around them up to date.
@MainActor
@Observable
public final class AppModel {
    public enum Phase: Equatable {
        case starting
        case onboarding
        case ready
        case locked
    }

    public let core: CoreService
    public let prefs: Preferences
    public var phase: Phase = .starting
    public var profile: Profile?
    public var connection: ConnectionStatus = .offline
    public var chats: [Chat] = []
    public var selectedChatID: ChatID? {
        didSet {
            guard oldValue != selectedChatID else { return }
            if let id = selectedChatID { conversation(for: id).open() }
        }
    }
    public var filter: ChatFilter = .all
    public var searchText = ""
    public var searchHits: [SearchHit] = []
    public var typing: [ChatID: Set<UserID>] = [:]
    public var contacts: [UserID: Contact] = [:]
    public var toast: Toast?
    public var incomingCall: IncomingCall?
    public var activeCall: CallSession?
    public var callKeys: [CallID: (String, UInt32)] = [:]
    public var showDetails = true
    public var sheet: AppSheet?
    public var quickSwitcherOpen = false
    public var settings = AppSettings()
    public var updateAvailable: UpdateInfo?
    public var installingUpdate = false
    /// Whether Asiri is the frontmost app ("hide chats when Asiri isn't in front", auto-lock).
    public var isActive = true
    public var pairingCompleted: (chat: ChatID, user: UserID)?

    @ObservationIgnored public private(set) var conversations: [ChatID: ConversationModel] = [:]
    @ObservationIgnored public var notifier: Notifier?
    @ObservationIgnored private var eventTask: Task<Void, Never>?
    @ObservationIgnored private var typingExpiry: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var toastTask: Task<Void, Never>?
    @ObservationIgnored public var onPanicWipe: (() -> Void)?
    @ObservationIgnored private var inactiveSince: Date?

    public init(core: CoreService, prefs: Preferences = .shared) {
        self.core = core
        self.prefs = prefs
    }

    // MARK: Lifecycle

    /// Loads the profile, decides between onboarding and the main window, and starts listening.
    public func start() {
        reloadProfile()
        settings = (try? core.settings()) ?? AppSettings()
        phase = (profile?.isRegistered ?? false) ? .ready : .onboarding
        connection = core.connectionStatus()
        refreshChats()
        refreshContacts()
        listen()
        if profile?.isRegistered == true {
            try? core.connect()
        }
        if selectedChatID == nil, let first = chats.first(where: { !$0.isArchived }) {
            selectedChatID = first.id
        }
    }

    private func listen() {
        eventTask?.cancel()
        let stream = core.events
        eventTask = Task { [weak self] in
            for await event in stream {
                guard let self else { return }
                self.handle(event)
            }
        }
    }

    public func reloadProfile() {
        profile = try? core.profile()
    }

    /// Called when onboarding finished (the Mac is registered).
    public func finishOnboarding() {
        reloadProfile()
        phase = .ready
        refreshChats()
        refreshContacts()
        try? core.connect()
        if selectedChatID == nil { selectedChatID = chats.first?.id }
    }

    // MARK: Events

    func handle(_ event: CoreEvent) {
        switch event {
        case let .connection(state):
            connection = state
        case .chatsChanged:
            refreshChats()
        case let .chatUpdated(id):
            refreshChat(id)
            conversations[id]?.reloadChat()
        case let .messageAdded(chat, message):
            conversations[chat]?.insert(message)
        case let .messageUpdated(chat, message):
            conversations[chat]?.refresh(message)
        case let .messageRemoved(chat, message):
            conversations[chat]?.remove(message)
            refreshChats()
        case let .typing(chat, user, active):
            setTyping(chat: chat, user: user, active: active)
        case let .notify(chat, message, sender):
            setTyping(chat: chat, user: sender, active: false)
            notify(chat: chat, message: message, sender: sender)
        case .contactsChanged:
            refreshContacts()
            refreshChats()
        case let .pairingCompleted(chat, user):
            pairingCompleted = (chat, user)
            refreshChats()
            refreshContacts()
        case let .incomingCall(call):
            guard activeCall == nil else { return }
            incomingCall = call
            notifier?.notifyCall(call, caller: name(of: call.caller), level: prefs.notificationLevel)
        case let .callUpdated(id):
            callUpdated(id)
        case let .callKeyRotated(call, key, index):
            callKeys[call] = (key, index)
        case let .securityWarning(_, text):
            show(.warning, text)
        case let .updateAvailable(version, notes, required):
            updateAvailable = UpdateInfo(version: version, build: 0, notes: notes, size: 0, isRequired: required)
        }
    }

    private func notify(chat: ChatID, message: MessageID, sender: UserID) {
        let isFrontmost = NSApp?.isActive ?? false
        let summary = chats.first { $0.id == chat }
        if isFrontmost, selectedChatID == chat { return }
        if summary?.isMuted == true { return }
        guard let m = try? core.message(message) else { return }
        notifier?.notifyMessage(chat: chat, chatTitle: summary?.title ?? "Asiri", sender: name(of: sender), text: m.preview,
                                level: prefs.notificationLevel)
    }

    private func setTyping(chat: ChatID, user: UserID, active: Bool) {
        let key = chat.base64EncodedString() + user.base64EncodedString()
        typingExpiry[key]?.cancel()
        if active {
            typing[chat, default: []].insert(user)
            // Typing indicators expire after 10 seconds (§12).
            typingExpiry[key] = Task { [weak self] in
                try? await Task.sleep(for: .seconds(10))
                guard !Task.isCancelled else { return }
                self?.typing[chat]?.remove(user)
            }
        } else {
            typing[chat]?.remove(user)
        }
    }

    private func callUpdated(_ id: CallID) {
        guard let record = try? core.call(id) else { return }
        if record.phase == .ended {
            if incomingCall?.callID == id { incomingCall = nil }
            notifier?.clearCall(id)
            if activeCall?.callID == id, !record.isJoined { activeCall = nil }
        }
    }

    // MARK: Data

    public func refreshChats() {
        chats = (try? core.chats()) ?? []
        notifier?.setBadge(totalUnread)
    }

    public func refreshChat(_ id: ChatID) {
        guard let updated = try? core.chat(id) else {
            chats.removeAll { $0.id == id }
            return
        }
        if let i = chats.firstIndex(where: { $0.id == id }) { chats[i] = updated } else { refreshChats() }
    }

    public func refreshContacts() {
        let list = (try? core.contacts()) ?? []
        contacts = Dictionary(uniqueKeysWithValues: list.map { ($0.id, $0) })
    }

    public func conversation(for id: ChatID) -> ConversationModel {
        if let c = conversations[id] { return c }
        let c = ConversationModel(chatID: id, app: self)
        conversations[id] = c
        return c
    }

    public var selectedChat: Chat? {
        guard let id = selectedChatID else { return nil }
        return chats.first { $0.id == id }
    }

    /// The chat list after the filter and search box.
    public var visibleChats: [Chat] {
        let base: [Chat]
        switch filter {
        case .all: base = chats.filter { !$0.isArchived }
        case .unread: base = chats.filter { !$0.isArchived && $0.unread > 0 }
        case .groups: base = chats.filter { !$0.isArchived && $0.kind == .group }
        case .archived: base = chats.filter(\.isArchived)
        }
        let q = searchText.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return base }
        return base.filter { $0.title.localizedCaseInsensitiveContains(q) }
    }

    public var totalUnread: Int {
        chats.filter { !$0.isMuted }.reduce(0) { $0 + $1.unread }
    }

    /// Runs a full-text search inside the encrypted database.
    public func runSearch() {
        let q = searchText.trimmingCharacters(in: .whitespaces)
        searchHits = q.count >= 2 ? ((try? core.search(q, limit: 50)) ?? []) : []
    }

    public func name(of user: UserID) -> String {
        if user == profile?.userID { return "You" }
        if let c = contacts[user], !c.name.isEmpty { return c.name }
        return "Someone"
    }

    public func contact(_ user: UserID) -> Contact? { contacts[user] }

    // MARK: Actions

    public func select(_ chat: ChatID) {
        selectedChatID = chat
    }

    public func selectNext(_ forward: Bool) {
        let list = visibleChats
        guard !list.isEmpty else { return }
        let i = list.firstIndex { $0.id == selectedChatID } ?? -1
        let next = forward ? min(list.count - 1, i + 1) : max(0, i - 1)
        selectedChatID = list[next].id
    }

    public func toggle(_ flag: ChatFlag, for chat: Chat) {
        let value: Bool
        switch flag {
        case .muted: value = !chat.isMuted
        case .pinned: value = !chat.isPinned
        case .archived: value = !chat.isArchived
        }
        perform { try self.core.setFlag(flag, value, for: chat.id) }
    }

    public func deleteLocally(_ chat: Chat) {
        perform {
            try self.core.deleteChatLocally(chat.id)
            if self.selectedChatID == chat.id { self.selectedChatID = nil }
            self.conversations[chat.id] = nil
        }
    }

    public func startChat(with user: UserID) async {
        do {
            let id = try await core.createChat(kind: .direct, name: "", members: [user])
            refreshChats()
            selectedChatID = id
        } catch {
            show(error)
        }
    }

    public func createGroup(name: String, members: [UserID]) async -> Bool {
        do {
            let id = try await core.createChat(kind: .group, name: name, members: members)
            refreshChats()
            selectedChatID = id
            show(.success, "“\(name)” is ready")
            return true
        } catch {
            show(error)
            return false
        }
    }

    public func saveSettings(_ s: AppSettings) {
        settings = s
        perform { try self.core.setSettings(s) }
    }

    // MARK: Calls

    public func startCall(in chat: ChatID, video: Bool) async {
        guard activeCall == nil else { return }
        do {
            activeCall = try await core.startCall(in: chat, video: video)
        } catch {
            show(error)
        }
    }

    public func answer(_ call: IncomingCall) async {
        incomingCall = nil
        notifier?.clearCall(call.callID)
        do {
            activeCall = try await core.acceptCall(call.callID)
        } catch {
            show(error)
        }
    }

    public func decline(_ call: IncomingCall) {
        incomingCall = nil
        notifier?.clearCall(call.callID)
        perform { try self.core.declineCall(call.callID) }
    }

    /// The name to put on a call tile for a LiveKit participant (a random alias): whoever the
    /// call signals said it is, or "Someone" until they have.
    public func participantName(alias: String, in call: CallID) -> String {
        guard let user = try? core.callParticipant(call, alias: alias) else { return "Someone" }
        return name(of: user)
    }

    public func hangUp() {
        guard let call = activeCall else { return }
        activeCall = nil
        perform { try self.core.endCall(call.callID) }
    }

    // MARK: Updates

    /// Downloads the update (over the relay, hash-checked against the signed manifest by the
    /// core) and opens the disk image, so the user drags the new Asiri into Applications (§16.2).
    public func installUpdate() async {
        guard !installingUpdate else { return }
        installingUpdate = true
        defer { installingUpdate = false }
        do {
            let image = try await core.downloadUpdate()
            NSWorkspace.shared.open(image)
            show(.success, "Drag Asiri into Applications to finish updating")
        } catch {
            show(error)
        }
    }

    // MARK: Lock and wipe

    /// Asiri went to the background.
    public func didResignActive(at now: Date = Date()) {
        isActive = false
        inactiveSince = now
    }

    /// Asiri came back to the front: lock first if it was away longer than the auto-lock time
    /// (Settings › Security), so nothing shows before the lock screen.
    public func didBecomeActive(at now: Date = Date()) {
        defer { inactiveSince = nil }
        if let since = inactiveSince, prefs.autoLockMinutes > 0,
           now.timeIntervalSince(since) >= Double(prefs.autoLockMinutes) * 60 {
            lock()
        }
        isActive = true
    }

    /// Whether there's anything to lock with: app lock or a PIN.
    public var canLock: Bool { prefs.appLockEnabled || PINStore().hasPIN }

    public func lock() {
        guard canLock, phase == .ready else { return }
        phase = .locked
    }

    public func unlocked() {
        phase = (profile?.isRegistered ?? false) ? .ready : .onboarding
    }

    /// Erases everything: the database key first (crypto-shredding), then the files (§6.6).
    public func panicWipe() async {
        KeyVault().destroy()
        PINStore().clear()
        try? await core.panicWipe()
        DataDirectory.erase()
        notifier?.clearAll()
        chats = []
        conversations = [:]
        selectedChatID = nil
        onPanicWipe?()
    }

    // MARK: Feedback

    public func show(_ style: Toast.Style, _ text: String) {
        let t = Toast(style, text)
        toast = t
        toastTask?.cancel()
        toastTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(style == .error || style == .warning ? 5 : 3))
            guard !Task.isCancelled else { return }
            if self?.toast?.id == t.id { self?.toast = nil }
        }
    }

    public func show(_ error: Error) {
        show(.error, (error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
    }

    /// Runs a throwing action and reports failures as a toast.
    public func perform(_ action: () throws -> Void) {
        do { try action() } catch { show(error) }
    }
}
