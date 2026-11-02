import AsiriDesign
import AsiriDomain
import SwiftUI

/// The sidebar: search, filters and the chats (§13.3).
struct ChatListView: View {
    @Bindable var app: AppModel
    @FocusState private var searchFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            header
            filterBar
            if !app.searchText.isEmpty, !app.searchHits.isEmpty {
                searchResults
            } else {
                list
            }
            footer
        }
        .background(Palette.sidebar)
    }

    private var header: some View {
        HStack(spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                    .font(.system(size: 12, weight: .semibold))
                TextField("Search", text: $app.searchText)
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .focused($searchFocused)
                    .onChange(of: app.searchText) { app.runSearch() }
                    .onExitCommand { app.searchText = ""; searchFocused = false }
                if !app.searchText.isEmpty {
                    Button { app.searchText = "" } label: {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                    }
                    .buttonStyle(.plain)
                    .transition(.scale.combined(with: .opacity))
                }
            }
            .padding(.horizontal, 9)
            .frame(height: 30)
            .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Palette.hairline))
            .animation(Motion.snappy, value: app.searchText.isEmpty)

            Menu {
                Button("New Chat", systemImage: "square.and.pencil") { app.sheet = .newChat }
                    .keyboardShortcut("n")
                Button("New Group", systemImage: "person.3") { app.sheet = .newGroup }
                    .keyboardShortcut("n", modifiers: [.command, .shift])
                Divider()
                Button("Add a Friend in Person…", systemImage: "qrcode.viewfinder") { app.sheet = .addFriend }
            } label: {
                Image(systemName: "square.and.pencil")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Palette.accent)
                    .frame(width: 30, height: 30)
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .frame(width: 30)
            .help("New chat")
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
        .padding(.bottom, 6)
    }

    private var filterBar: some View {
        HStack(spacing: 4) {
            ForEach(ChatFilter.allCases.filter { $0 != .archived || app.chats.contains(where: \.isArchived) }) { f in
                let selected = app.filter == f
                Button {
                    withAnimation(Motion.snappy) { app.filter = f }
                } label: {
                    HStack(spacing: 4) {
                        Text(f.rawValue)
                        if f == .unread, app.totalUnread > 0 {
                            Text("\(app.totalUnread)")
                                .font(.system(size: 10, weight: .bold))
                                .padding(.horizontal, 5)
                                .padding(.vertical, 1)
                                .background(Capsule().fill(selected ? Color.white.opacity(0.25) : Palette.accent.opacity(0.15)))
                        }
                    }
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(selected ? Color.white : Color.secondary)
                    .padding(.horizontal, 10)
                    .frame(height: 24)
                    .background {
                        if selected {
                            Capsule().fill(Palette.accent)
                                .matchedGeometryEffect(id: "filter", in: filterNamespace)
                        }
                    }
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
            }
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 6)
    }

    @Namespace private var filterNamespace

    private var list: some View {
        List(selection: $app.selectedChatID) {
            ForEach(app.visibleChats) { chat in
                ChatRow(chat: chat, app: app)
                    .tag(chat.id)
                    .listRowInsets(EdgeInsets(top: 2, leading: 6, bottom: 2, trailing: 6))
                    .contextMenu { rowMenu(chat) }
                    .swipeActions(edge: .leading) {
                        Button { app.toggle(.pinned, for: chat) } label: {
                            Label(chat.isPinned ? "Unpin" : "Pin", systemImage: chat.isPinned ? "pin.slash" : "pin")
                        }
                        .tint(Palette.accent)
                    }
                    .swipeActions(edge: .trailing) {
                        Button { app.toggle(.archived, for: chat) } label: {
                            Label(chat.isArchived ? "Unarchive" : "Archive", systemImage: "archivebox")
                        }
                        .tint(.gray)
                        Button { app.toggle(.muted, for: chat) } label: {
                            Label(chat.isMuted ? "Unmute" : "Mute", systemImage: chat.isMuted ? "bell" : "bell.slash")
                        }
                        .tint(Palette.amber)
                    }
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .animation(Motion.standard, value: app.visibleChats.map(\.id))
        .overlay {
            if app.visibleChats.isEmpty {
                ContentUnavailableView {
                    Label(app.searchText.isEmpty ? emptyTitle : "No results", systemImage: app.searchText.isEmpty ? "bubble.left.and.bubble.right" : "magnifyingglass")
                } description: {
                    Text(app.searchText.isEmpty ? emptySubtitle : "No chats match “\(app.searchText)”.")
                }
            }
        }
    }

    private var emptyTitle: String {
        switch app.filter {
        case .all: "No chats yet"
        case .unread: "All caught up"
        case .groups: "No groups"
        case .archived: "Nothing archived"
        }
    }

    private var emptySubtitle: String {
        switch app.filter {
        case .all: "Add a friend in person to start."
        case .unread: "You've read everything."
        case .groups: "Create one with ⇧⌘N."
        case .archived: "Archived chats appear here."
        }
    }

    @ViewBuilder
    private func rowMenu(_ chat: Chat) -> some View {
        Button(chat.isPinned ? "Unpin" : "Pin", systemImage: chat.isPinned ? "pin.slash" : "pin") { app.toggle(.pinned, for: chat) }
        Button(chat.isMuted ? "Unmute" : "Mute", systemImage: chat.isMuted ? "bell" : "bell.slash") { app.toggle(.muted, for: chat) }
        Button(chat.isArchived ? "Unarchive" : "Archive", systemImage: "archivebox") { app.toggle(.archived, for: chat) }
        if chat.unread > 0 {
            Button("Mark as Read", systemImage: "checkmark.message") {
                app.perform { try app.core.markRead(chat.id) }
                app.refreshChat(chat.id)
            }
        }
        Divider()
        Button("Delete from This Mac", systemImage: "trash", role: .destructive) { app.deleteLocally(chat) }
    }

    private var searchResults: some View {
        List {
            Section("Messages") {
                ForEach(app.searchHits) { hit in
                    Button {
                        app.select(hit.chatID)
                        app.conversation(for: hit.chatID).highlighted = hit.messageID
                        app.searchText = ""
                    } label: {
                        SearchHitRow(hit: hit, chat: app.chats.first { $0.id == hit.chatID })
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
    }

    private var footer: some View {
        HStack(spacing: 8) {
            StatusDot(app.connection)
            Text(app.connection.label)
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(.secondary)
                .contentTransition(.opacity)
                .animation(Motion.standard, value: app.connection)
            Spacer()
            SettingsLink {
                Image(systemName: "gearshape")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help("Settings (⌘,)")
        }
        .padding(.horizontal, 14)
        .frame(height: 34)
        .overlay(alignment: .top) { Divider() }
    }
}

/// One chat in the list.
struct ChatRow: View {
    let chat: Chat
    let app: AppModel
    @Environment(\.backgroundProminence) private var prominence

    private var selected: Bool { prominence == .increased }

    var body: some View {
        HStack(spacing: 10) {
            Avatar(id: chat.peer ?? chat.id, name: chat.title, image: chat.avatar, size: Metrics.rowAvatar, isGroup: chat.kind == .group)
                .overlay(alignment: .bottomTrailing) {
                    if chat.timerSeconds > 0 {
                        Image(systemName: "timer")
                            .font(.system(size: 9, weight: .bold))
                            .foregroundStyle(.white)
                            .frame(width: 17, height: 17)
                            .background(Circle().fill(Palette.accent))
                            .overlay(Circle().stroke(Palette.sidebar, lineWidth: 2))
                    }
                }
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 4) {
                    if chat.kind == .group {
                        Image(systemName: "person.2.fill")
                            .font(.system(size: 10))
                            .foregroundStyle(selected ? Color.white.opacity(0.85) : Color.secondary)
                    }
                    Text(chat.title)
                        .font(Typo.rowTitle)
                        .lineLimit(1)
                        .foregroundStyle(selected ? Color.white : Color.primary)
                    if let v = chat.verification { VerifiedBadge(v, compact: true) }
                    if chat.isMuted {
                        Image(systemName: "speaker.slash.fill")
                            .font(.system(size: 9.5))
                            .foregroundStyle(selected ? Color.white.opacity(0.7) : Color.secondary)
                    }
                    Spacer(minLength: 4)
                    if let last = chat.lastMessage, last.isOutgoing, last.isBubble {
                        StatusTicks(last.status, tint: selected ? .white.opacity(0.8) : .secondary, readTint: selected ? .white : Palette.accent)
                    }
                    Text(Formatting.chatListTime(chat.lastMessage?.sentAt ?? chat.lastActivity))
                        .font(.system(size: 11.5))
                        .foregroundStyle(selected ? Color.white.opacity(0.8) : (chat.unread > 0 && !chat.isMuted ? Palette.accent : Color.secondary))
                }
                HStack(alignment: .top, spacing: 4) {
                    preview
                    Spacer(minLength: 4)
                    if chat.unread > 0 {
                        UnreadBadge(count: chat.unread, muted: chat.isMuted)
                            .transition(.scale.combined(with: .opacity))
                    } else if chat.isPinned {
                        Image(systemName: "pin.fill")
                            .font(.system(size: 10))
                            .rotationEffect(.degrees(45))
                            .foregroundStyle(selected ? Color.white.opacity(0.7) : Color.secondary)
                    }
                }
                .animation(Motion.bouncy, value: chat.unread)
            }
        }
        .padding(.vertical, 5)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var preview: some View {
        let typers = app.typing[chat.id] ?? []
        if !typers.isEmpty {
            HStack(spacing: 5) {
                TypingDots(color: selected ? .white : Palette.accent)
                Text(chat.kind == .group ? "\(app.name(of: typers.first!).components(separatedBy: " ").first ?? "") is typing" : "typing")
                    .font(Typo.rowPreview)
                    .foregroundStyle(selected ? Color.white : Palette.accent)
                    .lineLimit(1)
            }
            .transition(.opacity)
        } else if let last = chat.lastMessage {
            (senderPrefix(last) + Text(last.preview).foregroundColor(selected ? .white.opacity(0.85) : .secondary))
                .font(Typo.rowPreview)
                .lineLimit(2)
                .transition(.opacity)
        } else {
            Text(chat.hasLeft ? "You left" : "No messages yet")
                .font(Typo.rowPreview)
                .foregroundStyle(selected ? Color.white.opacity(0.85) : Color.secondary)
        }
    }

    private func senderPrefix(_ m: ChatMessage) -> Text {
        guard m.isBubble else { return Text("") }
        if m.isOutgoing, chat.kind == .group { return Text("You: ").foregroundColor(selected ? .white : .primary) }
        if let name = chat.lastSenderName, chat.kind == .group {
            return Text("\(name): ").foregroundColor(selected ? .white : .primary)
        }
        return Text("")
    }
}

/// A full-text search hit.
struct SearchHitRow: View {
    let hit: SearchHit
    let chat: Chat?

    var body: some View {
        HStack(spacing: 10) {
            Avatar(id: chat?.peer ?? hit.chatID, name: chat?.title ?? "", image: chat?.avatar, size: 36, isGroup: chat?.kind == .group)
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(chat?.title ?? "Chat").font(Typo.rowTitle).lineLimit(1)
                    Spacer()
                    Text(Formatting.chatListTime(hit.sentAt)).font(.system(size: 11)).foregroundStyle(.secondary)
                }
                highlighted(hit.snippet)
                    .font(.system(size: 12.5))
                    .lineLimit(2)
            }
        }
        .padding(.vertical, 3)
    }

    /// Bolds the part between `[` and `]`.
    private func highlighted(_ snippet: String) -> Text {
        var result = Text("")
        var rest = Substring(snippet)
        while let open = rest.firstIndex(of: "["), let close = rest[open...].firstIndex(of: "]") {
            result = result + Text(rest[..<open]).foregroundColor(.secondary)
            result = result + Text(rest[rest.index(after: open) ..< close]).bold().foregroundColor(Palette.accent)
            rest = rest[rest.index(after: close)...]
        }
        return result + Text(rest).foregroundColor(.secondary)
    }
}
