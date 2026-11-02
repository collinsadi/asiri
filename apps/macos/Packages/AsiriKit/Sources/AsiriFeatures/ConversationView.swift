import AsiriDesign
import AsiriDomain
import SwiftUI
import UniformTypeIdentifiers

/// A conversation: header, timeline and composer (§13.3).
struct ConversationView: View {
    @Bindable var model: ConversationModel
    let app: AppModel
    @State private var atBottom = true
    @State private var dropTargeted = false
    @State private var viewer: ChatMessage?

    var body: some View {
        VStack(spacing: 0) {
            ConversationHeader(model: model, app: app)
            ZStack(alignment: .bottomTrailing) {
                if app.prefs.showWallpaper { ChatWallpaper() } else { Palette.wallpaperTop.ignoresSafeArea() }
                timeline
                if !atBottom {
                    JumpToLatestButton(count: model.newBelow) {
                        model.newBelow = 0
                        scrollRequest += 1
                    }
                    .padding(16)
                    .transition(.scale(scale: 0.5).combined(with: .opacity))
                }
            }
            .animation(Motion.bouncy, value: atBottom)
            .overlay {
                if dropTargeted {
                    DropOverlay()
                        .transition(.opacity)
                }
            }
            .animation(Motion.quick, value: dropTargeted)
            ComposerView(model: model, app: app)
        }
        .dropDestination(for: URL.self) { urls, _ in
            Task { await model.send(files: urls) }
            return !urls.isEmpty
        } isTargeted: { dropTargeted = $0 }
        .sheet(item: $viewer) { m in
            MediaViewer(message: m, model: model)
        }
        .onAppear { model.open() }
    }

    @State private var scrollRequest = 0

    private var timeline: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 0) {
                    if model.hasMoreHistory {
                        ProgressView()
                            .controlSize(.small)
                            .frame(height: 36)
                            .onAppear { loadOlder(proxy) }
                    } else {
                        EncryptionNotice(kind: model.chat?.kind ?? .direct)
                            .padding(.top, 18)
                            .padding(.bottom, 8)
                    }
                    ForEach(model.rows) { row in
                        rowView(row)
                            .id(row.id)
                    }
                    if model.isTyping {
                        TypingRow()
                            .id("typing")
                            .transition(.scale(scale: 0.6, anchor: .bottomLeading).combined(with: .opacity))
                    }
                    Color.clear
                        .frame(height: 10)
                        .id("bottom")
                        .onAppear { atBottom = true; model.isAtBottom = true; model.newBelow = 0 }
                        .onDisappear { atBottom = false; model.isAtBottom = false }
                }
                .padding(.horizontal, 14)
                .animation(Motion.standard, value: model.rows.map(\.id))
                .animation(Motion.bouncy, value: model.isTyping)
            }
            .defaultScrollAnchor(.bottom)
            .scrollContentBackground(.hidden)
            .onAppear {
                // Open at the newest message, or, when more arrived than a screen holds, at the
                // first unread one, like Telegram. Lazy rows settle their heights after the first
                // pass, so aim again once they have.
                let manyUnread = model.unreadAtOpen > 8 && model.rows.contains { $0.id == "unread" }
                let opening = manyUnread ? ("unread", UnitPoint.top) : ("bottom", UnitPoint.bottom)
                proxy.scrollTo(opening.0, anchor: opening.1)
                Task { @MainActor in
                    try? await Task.sleep(for: .milliseconds(80))
                    proxy.scrollTo(opening.0, anchor: opening.1)
                }
            }
            .onChange(of: model.sentCounter) {
                withAnimation(Motion.standard) { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .onChange(of: scrollRequest) {
                withAnimation(Motion.standard) { proxy.scrollTo("bottom", anchor: .bottom) }
            }
            .onChange(of: model.rows.last?.id) {
                if atBottom {
                    withAnimation(Motion.standard) { proxy.scrollTo("bottom", anchor: .bottom) }
                }
            }
            .onChange(of: model.highlighted) {
                guard let id = model.highlighted else { return }
                withAnimation(Motion.standard) { proxy.scrollTo(id.base64EncodedString(), anchor: .center) }
                Task {
                    try? await Task.sleep(for: .seconds(1.6))
                    if model.highlighted == id { model.highlighted = nil }
                }
            }
        }
    }

    private func loadOlder(_ proxy: ScrollViewProxy) {
        let anchor = model.rows.first?.id
        model.loadOlder()
        if let anchor {
            DispatchQueue.main.async { proxy.scrollTo(anchor, anchor: .top) }
        }
    }

    @ViewBuilder
    private func rowView(_ row: TimelineRow) -> some View {
        switch row {
        case let .day(date):
            CenterPill(Formatting.daySeparator(date))
                .padding(.vertical, 10)
                .frame(maxWidth: .infinity)
        case let .unreadMarker(count):
            UnreadMarker(count: count)
        case let .message(m, grouping):
            switch m.kind {
            case .system:
                CenterPill(m.body, icon: m.body.contains("may not have arrived") || m.body.contains("can't be read") ? "exclamationmark.shield" : nil)
                    .padding(.vertical, 6)
                    .frame(maxWidth: .infinity)
                    .transition(.opacity.combined(with: .scale(scale: 0.95)))
            default:
                MessageRow(message: m, grouping: grouping, model: model, app: app, openMedia: { viewer = $0 })
                    .transition(Motion.messageInsertion(outgoing: m.isOutgoing))
            }
        }
    }
}

// MARK: - Header

struct ConversationHeader: View {
    let model: ConversationModel
    let app: AppModel

    var body: some View {
        HStack(spacing: 10) {
            if let chat = model.chat {
                Avatar(id: chat.peer ?? chat.id, name: chat.title, image: chat.avatar, size: 34, isGroup: chat.kind == .group)
                VStack(alignment: .leading, spacing: 1) {
                    HStack(spacing: 5) {
                        Text(chat.title)
                            .font(Typo.title)
                            .lineLimit(1)
                        if let v = chat.verification { VerifiedBadge(v) }
                        if chat.timerSeconds > 0 {
                            Label(DisappearingTimer.label(chat.timerSeconds), systemImage: "timer")
                                .font(.system(size: 10.5, weight: .semibold))
                                .foregroundStyle(Palette.accent)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(Capsule().fill(Palette.accent.opacity(0.12)))
                        }
                    }
                    HStack(spacing: 4) {
                        if model.isTyping { TypingDots().scaleEffect(0.8) }
                        Text(model.subtitle)
                            .font(.system(size: 11.5))
                            .lineLimit(1)
                            .truncationMode(.tail)
                            .foregroundStyle(model.isTyping ? Palette.accent : (chat.verification == .changed ? Palette.danger : Color.secondary))
                            .contentTransition(.opacity)
                    }
                    .animation(Motion.standard, value: model.subtitle)
                }
                Spacer()
                if !chat.hasLeft {
                    CircleIconButton("phone") { Task { await app.startCall(in: chat.id, video: false) } }
                        .help("Voice call")
                        .disabled(app.activeCall != nil)
                    CircleIconButton("video") { Task { await app.startCall(in: chat.id, video: true) } }
                        .help("Video call")
                        .disabled(app.activeCall != nil)
                }
                CircleIconButton(app.showDetails ? "sidebar.right" : "sidebar.right") {
                    withAnimation(Motion.standard) { app.showDetails.toggle() }
                }
                .help("Show or hide details (⌥⌘I)")
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 54)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }
}

// MARK: - Small pieces

struct EncryptionNotice: View {
    let kind: ChatKind

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: "lock.fill")
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(Palette.amber)
            Text(kind == .direct ? "Only the two of you can read this chat." : "Only the members can read this group.")
                .font(.system(size: 12, weight: .semibold))
            Text("Messages are end-to-end encrypted with post-quantum keys. Not even the server can read them.")
                .font(.system(size: 11.5))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .padding(16)
        .frame(maxWidth: 300)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(.regularMaterial))
        .frame(maxWidth: .infinity)
    }
}

struct UnreadMarker: View {
    let count: Int

    var body: some View {
        Text(count == 1 ? "1 unread message" : "\(count) unread messages")
            .font(Typo.caption)
            .foregroundStyle(Palette.accent)
            .padding(.vertical, 5)
            .frame(maxWidth: .infinity)
            .background(Palette.accent.opacity(0.08))
            .padding(.vertical, 8)
            .padding(.horizontal, -14)
    }
}

struct TypingRow: View {
    var body: some View {
        HStack {
            TypingDots(color: Palette.incomingSecondary)
                .padding(.horizontal, 14)
                .padding(.vertical, 11)
                .background(BubbleBackground(outgoing: false, tail: true, groupedAbove: false, groupedBelow: false))
            Spacer()
        }
        .padding(.top, 8)
    }
}

struct JumpToLatestButton: View {
    let count: Int
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "chevron.down")
                .font(.system(size: 14, weight: .bold))
                .foregroundStyle(Palette.accent)
                .frame(width: 40, height: 40)
                .background(Circle().fill(.regularMaterial))
                .overlay(Circle().stroke(Palette.hairline))
                .shadow(color: .black.opacity(0.15), radius: 8, y: 3)
                .overlay(alignment: .top) {
                    if count > 0 {
                        UnreadBadge(count: count)
                            .offset(y: -10)
                            .transition(.scale.combined(with: .opacity))
                    }
                }
        }
        .buttonStyle(PressableStyle())
        .animation(Motion.bouncy, value: count)
        .help("Jump to latest")
    }
}

struct DropOverlay: View {
    var body: some View {
        ZStack {
            Palette.accent.opacity(0.12)
            RoundedRectangle(cornerRadius: 20, style: .continuous)
                .strokeBorder(Palette.accent, style: StrokeStyle(lineWidth: 2.5, dash: [9, 6]))
                .padding(18)
            VStack(spacing: 10) {
                Image(systemName: "arrow.down.doc.fill")
                    .font(.system(size: 34))
                    .repeatingBounce()
                Text("Drop to send, encrypted")
                    .font(.system(size: 15, weight: .semibold))
            }
            .foregroundStyle(Palette.accent)
        }
        .allowsHitTesting(false)
    }
}
