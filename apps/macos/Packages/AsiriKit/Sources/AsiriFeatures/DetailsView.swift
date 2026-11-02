import AppKit
import AsiriDesign
import AsiriDomain
import SwiftUI

/// The details pane (⌥⌘I): who's in the chat, verification, timer, media, and leaving.
struct DetailsView: View {
    let model: ConversationModel
    let app: AppModel
    @State private var confirmLeave = false
    @State private var confirmDelete = false
    @State private var editingGroup = false

    var body: some View {
        ScrollView {
            if let chat = model.chat {
                VStack(spacing: 18) {
                    hero(chat)
                    actions(chat)
                    if chat.kind == .direct, let peer = chat.peer { verification(chat, peer: peer) }
                    if chat.kind == .group, !chat.description.isEmpty {
                        section("About") {
                            Text(chat.description).font(.system(size: 13)).frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                    timer(chat)
                    media
                    if chat.kind == .group { members(chat) }
                    danger(chat)
                }
                .padding(16)
            }
        }
        .background(Palette.sidebar)
        .confirmationDialog("Leave this group?", isPresented: $confirmLeave) {
            Button("Leave", role: .destructive) {
                Task {
                    do { try await app.core.leave(model.chatID) } catch { app.show(error) }
                }
            }
        } message: {
            Text("You'll stop receiving messages. If you're the last admin, someone else becomes admin first.")
        }
        .confirmationDialog("Delete this chat from your Mac?", isPresented: $confirmDelete) {
            Button("Delete", role: .destructive) { if let chat = model.chat { app.deleteLocally(chat) } }
        } message: {
            Text("The history is erased from this Mac only. Other people keep their copies.")
        }
        .sheet(isPresented: $editingGroup) {
            if let chat = model.chat { EditGroupSheet(app: app, chat: chat) }
        }
    }

    // MARK: Sections

    private func hero(_ chat: Chat) -> some View {
        VStack(spacing: 8) {
            Avatar(id: chat.peer ?? chat.id, name: chat.title, image: chat.avatar, size: 84, isGroup: chat.kind == .group)
                .overlay {
                    if chat.verification?.isVerified == true {
                        Circle().stroke(Palette.amber, lineWidth: 2.5).padding(-4)
                    }
                }
                .shadow(color: .black.opacity(0.12), radius: 10, y: 4)
            HStack(spacing: 5) {
                Text(chat.title).font(.system(size: 17, weight: .bold)).multilineTextAlignment(.center)
                if let v = chat.verification { VerifiedBadge(v) }
            }
            Text(chat.kind == .group ? "\(chat.memberCount) members" : verificationText(chat.verification))
                .font(.system(size: 12))
                .foregroundStyle(chat.verification == .changed ? Palette.danger : Color.secondary)
            if chat.kind == .group, chat.isAdmin {
                Button("Edit Group") { editingGroup = true }
                    .buttonStyle(SecondaryButtonStyle())
                    .controlSize(.small)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 8)
    }

    private func actions(_ chat: Chat) -> some View {
        HStack(spacing: 10) {
            ActionTile(icon: "phone.fill", title: "Call") { Task { await app.startCall(in: chat.id, video: false) } }
                .disabled(chat.hasLeft || app.activeCall != nil)
            ActionTile(icon: "video.fill", title: "Video") { Task { await app.startCall(in: chat.id, video: true) } }
                .disabled(chat.hasLeft || app.activeCall != nil)
            ActionTile(icon: chat.isMuted ? "bell.slash.fill" : "bell.fill", title: chat.isMuted ? "Unmute" : "Mute") {
                app.toggle(.muted, for: chat)
            }
            ActionTile(icon: chat.isPinned ? "pin.slash.fill" : "pin.fill", title: chat.isPinned ? "Unpin" : "Pin") {
                app.toggle(.pinned, for: chat)
            }
        }
    }

    private func verification(_ chat: Chat, peer: UserID) -> some View {
        section("Security") {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 10) {
                    Image(systemName: chat.verification?.isVerified == true ? "checkmark.seal.fill" : (chat.verification == .changed ? "exclamationmark.triangle.fill" : "seal"))
                        .font(.system(size: 20))
                        .foregroundStyle(chat.verification?.isVerified == true ? Palette.amber : (chat.verification == .changed ? Palette.danger : Color.secondary))
                    VStack(alignment: .leading, spacing: 2) {
                        Text(verificationText(chat.verification)).font(.system(size: 13, weight: .semibold))
                        Text(chat.verification == .changed
                            ? "\(chat.title)'s security key changed. Verify again before sharing anything sensitive."
                            : "Only you and \(chat.title) can read this chat.")
                            .font(.system(size: 11.5))
                            .foregroundStyle(.secondary)
                    }
                }
                Button {
                    app.sheet = .verify(peer)
                } label: {
                    Label("Compare safety number", systemImage: "number")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(SecondaryButtonStyle())
            }
        }
    }

    private func timer(_ chat: Chat) -> some View {
        section("Disappearing messages") {
            Picker(selection: Binding(
                get: { chat.timerSeconds },
                set: { s in app.perform { try app.core.setDisappearingTimer(s, for: chat.id) } }
            )) {
                ForEach(DisappearingTimer.choices, id: \.self) { s in
                    Text(DisappearingTimer.label(s)).tag(s)
                }
            } label: {
                Label("Timer", systemImage: "timer")
            }
            .pickerStyle(.menu)
            .disabled(chat.hasLeft)
            Text("New messages disappear for everyone after this time.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var media: some View {
        let photos = model.messages.filter { $0.attachment?.isImage == true && !$0.isDeleted }.suffix(9).reversed()
        if !photos.isEmpty {
            section("Shared media") {
                LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 4), count: 3), spacing: 4) {
                    ForEach(Array(photos)) { m in
                        PhotoView(message: m, attachment: m.attachment!, model: model)
                            .frame(height: 78)
                            .frame(maxWidth: .infinity)
                            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .onTapGesture { model.highlighted = m.id }
                    }
                }
            }
        }
    }

    private func members(_ chat: Chat) -> some View {
        section("\(model.members.count) members") {
            VStack(spacing: 2) {
                if chat.isAdmin, !chat.hasLeft {
                    Button { app.sheet = .addMembers(chat.id) } label: {
                        HStack(spacing: 10) {
                            Image(systemName: "person.badge.plus")
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundStyle(.white)
                                .frame(width: 32, height: 32)
                                .background(Circle().fill(Palette.accent))
                            Text("Add members").font(.system(size: 13, weight: .medium)).foregroundStyle(Palette.accent)
                            Spacer()
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .padding(.vertical, 4)
                }
                ForEach(model.members) { member in
                    MemberRow(member: member, chat: chat, app: app)
                }
            }
        }
    }

    private func danger(_ chat: Chat) -> some View {
        VStack(spacing: 8) {
            if chat.kind == .direct, let peer = chat.peer {
                let blocked = app.contact(peer)?.isBlocked == true
                Button {
                    app.perform { try app.core.setBlocked(peer, !blocked) }
                } label: {
                    Label(blocked ? "Unblock \(chat.title)" : "Block \(chat.title)", systemImage: "hand.raised.fill")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(DangerButtonStyle())
            }
            if chat.kind == .group, !chat.hasLeft {
                Button { confirmLeave = true } label: {
                    Label("Leave group", systemImage: "rectangle.portrait.and.arrow.right")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(DangerButtonStyle())
            }
            Button { confirmDelete = true } label: {
                Label("Delete from this Mac", systemImage: "trash")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(DangerButtonStyle())
        }
    }

    // MARK: Helpers

    private func section<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title.uppercased())
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundStyle(.secondary)
                .tracking(0.6)
            content()
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Palette.surface))
    }

    private func verificationText(_ v: Verification?) -> String {
        switch v {
        case .inPerson: "Verified in person"
        case .remote: "Verified with safety number"
        case .changed: "Security key changed"
        default: "Not verified yet"
        }
    }
}

struct ActionTile: View {
    let icon: String
    let title: String
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(spacing: 5) {
                Image(systemName: icon)
                    .font(.system(size: 15, weight: .semibold))
                    .frame(height: 18)
                Text(title)
                    .font(.system(size: 11, weight: .medium))
            }
            .foregroundStyle(Palette.accent)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 9)
            .background(RoundedRectangle(cornerRadius: 11, style: .continuous).fill(Palette.accent.opacity(hovering ? 0.16 : 0.09)))
        }
        .buttonStyle(PressableStyle())
        .onHover { hovering = $0 }
        .animation(Motion.quick, value: hovering)
    }
}

struct DangerButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(Palette.danger)
            .padding(.vertical, 8)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Palette.danger.opacity(configuration.isPressed ? 0.16 : 0.08)))
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(Motion.snappy, value: configuration.isPressed)
    }
}

struct MemberRow: View {
    let member: Member
    let chat: Chat
    let app: AppModel

    var body: some View {
        HStack(spacing: 10) {
            Avatar(id: member.id, name: member.contact.displayName, image: member.contact.avatar, size: 32)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 4) {
                    Text(member.isMe ? "You" : member.contact.displayName)
                        .font(.system(size: 13, weight: .medium))
                        .lineLimit(1)
                    VerifiedBadge(member.contact.verification, compact: true)
                }
                if member.isAdmin {
                    Text("admin").font(.system(size: 11)).foregroundStyle(Palette.accent)
                }
            }
            Spacer()
        }
        .padding(.vertical, 3)
        .contentShape(Rectangle())
        .contextMenu {
            if !member.isMe {
                Button("Message", systemImage: "bubble.left") { Task { await app.startChat(with: member.id) } }
                if chat.isAdmin, !chat.hasLeft {
                    Button(member.isAdmin ? "Remove as Admin" : "Make Admin", systemImage: "star") {
                        Task {
                            do { try await app.core.setAdmin(member.id, !member.isAdmin, in: chat.id) } catch { app.show(error) }
                        }
                    }
                    Button("Remove from Group", systemImage: "person.badge.minus", role: .destructive) {
                        Task {
                            do { try await app.core.removeMember(member.id, from: chat.id) } catch { app.show(error) }
                        }
                    }
                }
            }
        }
    }
}
