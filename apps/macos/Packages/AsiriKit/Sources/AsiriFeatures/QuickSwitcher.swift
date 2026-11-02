import AsiriDesign
import AsiriDomain
import AsiriPlatform
import SwiftUI

/// ⌘K: jump to any chat or search every message (§13.3).
struct QuickSwitcher: View {
    let app: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var hits: [SearchHit] = []
    @State private var index = 0
    @FocusState private var focused: Bool

    private var chats: [Chat] {
        let q = query.trimmingCharacters(in: .whitespaces)
        let all = app.chats.filter { !$0.isArchived }
        guard !q.isEmpty else { return Array(all.prefix(8)) }
        return all.filter { $0.title.localizedCaseInsensitiveContains(q) }
    }

    private var count: Int { chats.count + hits.count }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(Palette.accent)
                TextField("Jump to a chat or search messages", text: $query)
                    .textFieldStyle(.plain)
                    .font(.system(size: 17))
                    .focused($focused)
                    .onSubmit(openSelected)
                    .onChange(of: query) {
                        index = 0
                        let q = query.trimmingCharacters(in: .whitespaces)
                        hits = q.count >= 2 ? ((try? app.core.search(q, limit: 20)) ?? []) : []
                    }
                    .onKeyPress(.downArrow) { index = max(0, min(count - 1, index + 1)); return .handled }
                    .onKeyPress(.upArrow) { index = max(0, index - 1); return .handled }
                Text("esc").font(.system(size: 11, weight: .medium)).foregroundStyle(.tertiary)
            }
            .padding(16)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    if !chats.isEmpty {
                        header("Chats")
                        ForEach(Array(chats.enumerated()), id: \.element.id) { i, chat in
                            row(selected: i == index) {
                                HStack(spacing: 10) {
                                    Avatar(id: chat.peer ?? chat.id, name: chat.title, image: chat.avatar, size: 28, isGroup: chat.kind == .group)
                                    Text(chat.title).font(.system(size: 13.5, weight: .medium))
                                    if let v = chat.verification { VerifiedBadge(v, compact: true) }
                                    Spacer()
                                    if chat.unread > 0 { UnreadBadge(count: chat.unread, muted: chat.isMuted) }
                                }
                            } action: { open(chat.id, message: nil) }
                        }
                    }
                    if !hits.isEmpty {
                        header("Messages")
                        ForEach(Array(hits.enumerated()), id: \.element.id) { i, hit in
                            row(selected: chats.count + i == index) {
                                SearchHitRow(hit: hit, chat: app.chats.first { $0.id == hit.chatID })
                            } action: { open(hit.chatID, message: hit.messageID) }
                        }
                    }
                    if chats.isEmpty, hits.isEmpty {
                        Text(query.isEmpty ? "No chats yet" : "Nothing found for “\(query)”")
                            .font(.system(size: 13))
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity)
                            .padding(30)
                    }
                }
                .padding(8)
            }
        }
        .frame(width: 560, height: 420)
        .onAppear { focused = true }
    }

    private func header(_ text: String) -> some View {
        Text(text.uppercased())
            .font(.system(size: 10.5, weight: .semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 10)
            .padding(.top, 8)
            .padding(.bottom, 2)
    }

    private func row<Content: View>(selected: Bool, @ViewBuilder content: () -> Content, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            content()
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(selected ? Palette.accent.opacity(0.14) : Color.clear))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func openSelected() {
        guard index >= 0 else { return }
        if index < chats.count {
            open(chats[index].id, message: nil)
        } else if index - chats.count < hits.count {
            let hit = hits[index - chats.count]
            open(hit.chatID, message: hit.messageID)
        }
    }

    private func open(_ chat: ChatID, message: MessageID?) {
        app.select(chat)
        if let message { app.conversation(for: chat).highlighted = message }
        dismiss()
    }
}

/// When app lock is on: Touch ID or password, or the app PIN (with the duress PIN).
struct LockView: View {
    let app: AppModel
    @State private var pin = ""
    @State private var shake = 0
    @State private var authenticating = false

    var body: some View {
        ZStack {
            OnboardingBackground()
            VStack(spacing: 20) {
                AsiriMark(size: 80)
                    .shadow(color: Palette.indigoBright.opacity(0.5), radius: 24)
                Text("Asiri is locked")
                    .font(.system(size: 22, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
                if app.prefs.appLockEnabled {
                    Button {
                        unlockWithSystem()
                    } label: {
                        Label("Unlock with Touch ID", systemImage: "touchid")
                    }
                    .buttonStyle(PrimaryButtonStyle(tint: Palette.indigoBright))
                    .keyboardShortcut(.defaultAction)
                    .disabled(authenticating)
                }
                if PINStore().hasPIN {
                    SecureField("PIN", text: $pin)
                        .textFieldStyle(.plain)
                        .font(.system(size: 20, weight: .semibold, design: .monospaced))
                        .multilineTextAlignment(.center)
                        .frame(width: 180)
                        .padding(10)
                        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color.white.opacity(0.1)))
                        .foregroundStyle(.white)
                        .onSubmit(checkPIN)
                        .modifier(Shake(animatableData: CGFloat(shake)))
                }
            }
        }
        .environment(\.colorScheme, .dark)
        .onAppear { if app.prefs.appLockEnabled { unlockWithSystem() } }
    }

    private func unlockWithSystem() {
        authenticating = true
        Task {
            let ok = await AppLock.authenticate(reason: "unlock your chats")
            authenticating = false
            if ok { app.unlocked() }
        }
    }

    private func checkPIN() {
        switch PINStore().check(pin) {
        case .unlock:
            app.unlocked()
        case .duress:
            // Looks like a normal unlock; behind the scenes everything is erased (§13.3 Security).
            Task { await app.panicWipe() }
        case .wrong:
            withAnimation(.default) { shake += 1 }
            pin = ""
        }
    }
}

struct Shake: GeometryEffect {
    var animatableData: CGFloat

    func effectValue(size: CGSize) -> ProjectionTransform {
        ProjectionTransform(CGAffineTransform(translationX: 10 * sin(animatableData * .pi * 4), y: 0))
    }
}
