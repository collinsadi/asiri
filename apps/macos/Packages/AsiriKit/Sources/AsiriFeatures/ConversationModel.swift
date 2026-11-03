import AppKit
import AsiriDomain
import AsiriPlatform
import Foundation
import Observation

/// How a bubble sits among its neighbours (Telegram-style grouping, ADR-0005).
public struct BubbleGrouping: Hashable, Sendable {
    /// The previous message is from the same sender within a few minutes.
    public var groupedAbove: Bool
    /// The next message is from the same sender within a few minutes.
    public var groupedBelow: Bool
    /// Show the sender's name (first incoming bubble of a group in group chats).
    public var showName: Bool
    /// Show the sender's avatar (last incoming bubble of a group in group chats).
    public var showAvatar: Bool

    public var hasTail: Bool { !groupedBelow }
}

/// One row of the timeline.
public enum TimelineRow: Identifiable, Hashable {
    case day(Date)
    case unreadMarker(Int)
    case message(ChatMessage, BubbleGrouping)

    public var id: String {
        switch self {
        case let .day(d): "day-\(Int(d.timeIntervalSince1970))"
        case .unreadMarker: "unread"
        case let .message(m, _): m.id.base64EncodedString()
        }
    }
}

/// One open conversation: its messages, the draft, replies and edits.
@MainActor
@Observable
public final class ConversationModel {
    public let chatID: ChatID
    unowned let app: AppModel

    public var chat: Chat?
    public private(set) var messages: [ChatMessage] = []
    public private(set) var rows: [TimelineRow] = []
    public var members: [Member] = []
    public var draft = "" {
        didSet { draftChanged(oldValue) }
    }
    public var replyingTo: ChatMessage?
    public var editing: ChatMessage?
    public var hasMoreHistory = true
    public var isLoadingOlder = false
    /// Message to flash after jumping to it (search, reply quote).
    public var highlighted: MessageID?
    /// Bumped whenever the user sends, so the timeline scrolls to the bottom.
    public var sentCounter = 0
    /// Messages that arrived while the user was scrolled up.
    public var newBelow = 0
    public var lastReaction: (MessageID, String)?

    @ObservationIgnored private var loaded = false
    @ObservationIgnored private(set) var unreadAtOpen = 0
    @ObservationIgnored private var firstUnreadID: MessageID?
    @ObservationIgnored private var typingTask: Task<Void, Never>?
    @ObservationIgnored public var isAtBottom = true

    static let pageSize = 80
    static let groupWindow: TimeInterval = 5 * 60

    init(chatID: ChatID, app: AppModel) {
        self.chatID = chatID
        self.app = app
    }

    private var core: CoreService { app.core }

    // MARK: Loading

    /// Called when the chat is selected.
    func open() {
        reloadChat()
        if !loaded {
            loaded = true
            unreadAtOpen = chat?.unread ?? 0
            messages = (try? core.messages(in: chatID, before: nil, limit: Self.pageSize)) ?? []
            hasMoreHistory = messages.count >= Self.pageSize
            if unreadAtOpen > 0 {
                let incoming = messages.filter { !$0.isOutgoing && $0.isBubble }
                firstUnreadID = incoming.suffix(unreadAtOpen).first?.id
            }
            rebuildRows()
        }
        markRead()
        if chat?.kind == .group || members.isEmpty {
            members = (try? core.members(of: chatID)) ?? []
        }
    }

    public func reloadChat() {
        chat = try? core.chat(chatID)
        if chat?.kind == .group {
            members = (try? core.members(of: chatID)) ?? members
        }
    }

    public func loadOlder() {
        guard hasMoreHistory, !isLoadingOlder, let oldest = messages.first else { return }
        isLoadingOlder = true
        let older = (try? core.messages(in: chatID, before: oldest.id, limit: Self.pageSize)) ?? []
        hasMoreHistory = older.count >= Self.pageSize
        messages.insert(contentsOf: older, at: 0)
        rebuildRows()
        isLoadingOlder = false
    }

    func markRead() {
        guard (chat?.unread ?? 0) > 0 || messages.contains(where: { $0.status == .received }) else { return }
        try? core.markRead(chatID)
        app.refreshChat(chatID)
    }

    // MARK: Events

    func insert(_ id: MessageID) {
        guard loaded, !messages.contains(where: { $0.id == id }), let m = try? core.message(id) else {
            app.refreshChat(chatID)
            return
        }
        let i = messages.firstIndex { $0.sortKey > m.sortKey } ?? messages.count
        messages.insert(m, at: i)
        if !m.isOutgoing, !isAtBottom { newBelow += 1 }
        rebuildRows()
        if app.selectedChatID == chatID, NSApp?.isActive ?? true, isAtBottom {
            markRead()
        } else {
            app.refreshChat(chatID)
        }
    }

    func refresh(_ id: MessageID) {
        guard let i = messages.firstIndex(where: { $0.id == id }), let m = try? core.message(id) else { return }
        messages[i] = m
        rebuildRows()
    }

    func remove(_ id: MessageID) {
        messages.removeAll { $0.id == id }
        rebuildRows()
    }

    // MARK: Timeline

    func rebuildRows() {
        var out: [TimelineRow] = []
        let calendar = Calendar.current
        let isGroup = chat?.kind == .group
        // A DM opens with the core's "only the two of you" line; at the start of history the
        // encryption card above the timeline already says it, more fully.
        let skipOpening = !hasMoreHistory && chat?.kind == .direct && messages.count > 1 && messages.first?.kind == .system
        let shown = skipOpening ? Array(messages.dropFirst()) : messages
        for (i, m) in shown.enumerated() {
            if i == 0 || !calendar.isDate(shown[i - 1].sentAt, inSameDayAs: m.sentAt) {
                out.append(.day(calendar.startOfDay(for: m.sentAt)))
            }
            if m.id == firstUnreadID {
                out.append(.unreadMarker(unreadAtOpen))
            }
            let prev = i > 0 ? shown[i - 1] : nil
            let next = i + 1 < shown.count ? shown[i + 1] : nil
            let above = prev.map { Self.joins($0, m, calendar) } ?? false
            let below = next.map { Self.joins(m, $0, calendar) && $0.id != firstUnreadID } ?? false
            let grouping = BubbleGrouping(
                groupedAbove: above,
                groupedBelow: below,
                showName: isGroup && !m.isOutgoing && !above,
                showAvatar: isGroup && !m.isOutgoing && !below
            )
            out.append(.message(m, grouping))
        }
        rows = out
    }

    /// Whether `b` continues `a`'s group: same sender, both bubbles, same day, close in time.
    static func joins(_ a: ChatMessage, _ b: ChatMessage, _ calendar: Calendar) -> Bool {
        a.isBubble && b.isBubble && a.sender == b.sender && a.isOutgoing == b.isOutgoing
            && calendar.isDate(a.sentAt, inSameDayAs: b.sentAt)
            && b.sentAt.timeIntervalSince(a.sentAt) < groupWindow
    }

    public func message(_ id: MessageID) -> ChatMessage? {
        messages.first { $0.id == id }
    }

    /// The message a reply quotes (loaded on demand if it's older than the page).
    public func quoted(_ id: MessageID) -> ChatMessage? {
        message(id) ?? (try? core.message(id))
    }

    // MARK: Composing

    private func draftChanged(_ old: String) {
        let active = !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        let wasActive = !old.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        guard app.settings.typingIndicators, editing == nil else { return }
        if active {
            let core = self.core
            let chatID = self.chatID
            try? core.setTyping(true, in: chatID)
            typingTask?.cancel()
            typingTask = Task { [weak self] in
                try? await Task.sleep(for: .seconds(5))
                guard !Task.isCancelled, self != nil else { return }
                try? core.setTyping(false, in: chatID)
            }
        } else if wasActive {
            typingTask?.cancel()
            try? core.setTyping(false, in: chatID)
        }
    }

    /// Sends the draft (or saves the edit). Returns false when there was nothing to send.
    @discardableResult
    public func send() -> Bool {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return false }
        if let editing {
            if text != editing.body {
                app.perform { try core.edit(editing.id, in: chatID, text: text) }
            }
            self.editing = nil
            draft = ""
            return true
        }
        do {
            _ = try core.sendText(text, in: chatID, replyTo: replyingTo?.id)
            draft = ""
            replyingTo = nil
            sentCounter += 1
            typingTask?.cancel()
            return true
        } catch {
            app.show(error)
            return false
        }
    }

    public func reply(to m: ChatMessage) {
        editing = nil
        replyingTo = m
    }

    public func beginEdit(_ m: ChatMessage) {
        guard m.isOutgoing, m.kind == .text, !m.isDeleted else { return }
        replyingTo = nil
        editing = m
        draft = m.body
    }

    /// ↑ in an empty composer edits the last message the user sent (§13.3).
    public func editLast() -> Bool {
        guard draft.isEmpty, let last = messages.last(where: { $0.isOutgoing && $0.kind == .text && !$0.isDeleted }) else { return false }
        beginEdit(last)
        return true
    }

    public func cancelCompose() {
        if editing != nil { draft = "" }
        editing = nil
        replyingTo = nil
    }

    public func toggleReaction(_ emoji: String, on m: ChatMessage) {
        let mine = m.reactions.contains { $0.emoji == emoji && $0.mine }
        if !mine { lastReaction = (m.id, emoji) }
        app.perform { try core.react(emoji, to: m.id, in: chatID, remove: mine) }
    }

    public func delete(_ m: ChatMessage, forEveryone: Bool) {
        app.perform {
            if forEveryone { try core.deleteForEveryone(m.id, in: chatID) } else { try core.deleteForMe(m.id, in: chatID) }
        }
    }

    public func retry(_ m: ChatMessage) {
        Task {
            do { try await core.retry(m.id) } catch { app.show(error) }
        }
    }

    public func copy(_ m: ChatMessage) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(m.body, forType: .string)
    }

    // MARK: Files

    public func send(files urls: [URL], caption: String = "") async {
        for url in urls {
            do {
                let file = try MediaPrep.prepare(url: url)
                _ = try await core.sendFile(file, caption: urls.count == 1 ? caption : "", in: chatID, replyTo: replyingTo?.id)
                sentCounter += 1
            } catch {
                app.show(error)
            }
        }
        replyingTo = nil
    }

    public func send(file: OutgoingFile, caption: String = "") async {
        do {
            _ = try await core.sendFile(file, caption: caption, in: chatID, replyTo: replyingTo?.id)
            replyingTo = nil
            sentCounter += 1
        } catch {
            app.show(error)
        }
    }

    public func download(_ m: ChatMessage) {
        Task {
            do { try await core.download(m.id) } catch { app.show(error) }
        }
    }

    public func data(of m: ChatMessage) -> Data? {
        try? core.attachmentData(m.id)
    }

    public func save(_ m: ChatMessage) {
        guard let a = m.attachment else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = a.fileName
        panel.message = "Saved files are no longer protected by Asiri."
        panel.begin { [weak self] response in
            guard response == .OK, let url = panel.url, let self else { return }
            Task { @MainActor in
                self.app.perform { try self.core.exportAttachment(m.id, to: url) }
            }
        }
    }

    // MARK: Header

    public var subtitle: String {
        guard let chat else { return "" }
        let typers = app.typing[chatID] ?? []
        if !typers.isEmpty {
            if chat.kind == .direct { return "typing…" }
            let names = typers.map { app.name(of: $0).components(separatedBy: " ").first ?? "" }
            return names.count == 1 ? "\(names[0]) is typing…" : "\(names.count) people are typing…"
        }
        if chat.hasLeft { return "You're no longer in this chat" }
        switch chat.kind {
        case .direct:
            switch chat.verification {
            case .inPerson: return "Verified in person · end-to-end encrypted"
            case .remote: return "Verified · end-to-end encrypted"
            case .changed: return "Security key changed · verify again"
            default: return "End-to-end encrypted"
            }
        case .group:
            return chat.memberCount == 1 ? "1 member" : "\(chat.memberCount) members"
        }
    }

    public var isTyping: Bool { !(app.typing[chatID] ?? []).isEmpty }
}
