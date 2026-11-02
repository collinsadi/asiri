import AppKit
import AsiriDesign
import AsiriDomain
import AsiriPlatform
import SwiftUI

/// ⌘N: a direct chat with someone already paired.
struct NewChatSheet: View {
    let app: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""

    private var people: [Contact] {
        let all = app.contacts.values.filter { !$0.isBlocked && $0.id != app.profile?.userID }.sorted { $0.displayName < $1.displayName }
        return query.isEmpty ? all : all.filter { $0.displayName.localizedCaseInsensitiveContains(query) }
    }

    var body: some View {
        SheetScaffold(title: "New chat", onClose: { dismiss() }) {
            SearchField(text: $query, placeholder: "Search people")
            if people.isEmpty {
                ContentUnavailableView {
                    Label("Nobody here yet", systemImage: "person.crop.circle.badge.plus")
                } description: {
                    Text("People appear here after you pair with them in person.")
                } actions: {
                    Button("Add a friend") { app.sheet = .addFriend }
                }
                .frame(maxHeight: .infinity)
            } else {
                List(people) { person in
                    Button {
                        Task {
                            await app.startChat(with: person.id)
                            dismiss()
                        }
                    } label: {
                        PersonRow(contact: person)
                    }
                    .buttonStyle(.plain)
                }
                .listStyle(.plain)
            }
            Button {
                app.sheet = .newGroup
            } label: {
                Label("New group", systemImage: "person.3.fill").frame(maxWidth: .infinity)
            }
            .buttonStyle(SecondaryButtonStyle())
        }
    }
}

/// ⇧⌘N: a group with a name and members.
struct NewGroupSheet: View {
    let app: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var selected: Set<UserID> = []
    @State private var query = ""
    @State private var working = false

    private var people: [Contact] {
        let all = app.contacts.values.filter { !$0.isBlocked && $0.id != app.profile?.userID }.sorted { $0.displayName < $1.displayName }
        return query.isEmpty ? all : all.filter { $0.displayName.localizedCaseInsensitiveContains(query) }
    }

    var body: some View {
        SheetScaffold(title: "New group", onClose: { dismiss() }) {
            HStack(spacing: 12) {
                Avatar(id: Data(name.utf8), name: name, size: 52, isGroup: true)
                TextField("Group name", text: $name)
                    .textFieldStyle(.plain)
                    .font(.system(size: 16, weight: .semibold))
                    .padding(10)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Palette.hairline))
            }
            if !selected.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(people.filter { selected.contains($0.id) }) { p in
                            HStack(spacing: 5) {
                                Avatar(id: p.id, name: p.displayName, image: p.avatar, size: 20)
                                Text(p.displayName).font(.system(size: 12, weight: .medium))
                                Button { selected.remove(p.id) } label: { Image(systemName: "xmark").font(.system(size: 9, weight: .bold)) }
                                    .buttonStyle(.plain)
                            }
                            .padding(.leading, 3)
                            .padding(.trailing, 8)
                            .padding(.vertical, 3)
                            .background(Capsule().fill(Palette.accent.opacity(0.14)))
                            .transition(.scale.combined(with: .opacity))
                        }
                    }
                }
                .animation(Motion.bouncy, value: selected)
            }
            SearchField(text: $query, placeholder: "Add people")
            List(people) { person in
                Button {
                    if selected.contains(person.id) { selected.remove(person.id) } else { selected.insert(person.id) }
                } label: {
                    HStack {
                        PersonRow(contact: person)
                        Image(systemName: selected.contains(person.id) ? "checkmark.circle.fill" : "circle")
                            .font(.system(size: 18))
                            .foregroundStyle(selected.contains(person.id) ? Palette.accent : Color.secondary)
                            .contentTransition(.symbolEffect(.replace))
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .listStyle(.plain)
            Button {
                working = true
                Task {
                    if await app.createGroup(name: name.trimmingCharacters(in: .whitespaces), members: Array(selected)) { dismiss() }
                    working = false
                }
            } label: {
                HStack {
                    if working { ProgressView().controlSize(.small) }
                    Text(selected.isEmpty ? "Create group" : "Create group with \(selected.count)")
                }
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(PrimaryButtonStyle())
            .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || working)
            .keyboardShortcut(.defaultAction)
        }
    }
}

/// Adding people to an existing group (admins).
struct AddMembersSheet: View {
    let app: AppModel
    let chatID: ChatID
    @Environment(\.dismiss) private var dismiss
    @State private var selected: Set<UserID> = []
    @State private var working = false

    private var candidates: [Contact] {
        let present = Set(app.conversation(for: chatID).members.map(\.id))
        return app.contacts.values.filter { !present.contains($0.id) && !$0.isBlocked && $0.id != app.profile?.userID }
            .sorted { $0.displayName < $1.displayName }
    }

    var body: some View {
        SheetScaffold(title: "Add members", onClose: { dismiss() }) {
            if candidates.isEmpty {
                ContentUnavailableView("Everyone you know is already here", systemImage: "person.3.fill")
                    .frame(maxHeight: .infinity)
            } else {
                List(candidates) { person in
                    Button {
                        if selected.contains(person.id) { selected.remove(person.id) } else { selected.insert(person.id) }
                    } label: {
                        HStack {
                            PersonRow(contact: person)
                            Image(systemName: selected.contains(person.id) ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(selected.contains(person.id) ? Palette.accent : Color.secondary)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
                .listStyle(.plain)
            }
            Text("New members can't read messages sent before they joined.")
                .font(.system(size: 11.5))
                .foregroundStyle(.secondary)
            Button {
                working = true
                Task {
                    do {
                        try await app.core.addMembers(Array(selected), to: chatID)
                        app.conversation(for: chatID).reloadChat()
                        dismiss()
                    } catch {
                        app.show(error)
                    }
                    working = false
                }
            } label: {
                HStack {
                    if working { ProgressView().controlSize(.small) }
                    Text("Add \(selected.count)")
                }
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(PrimaryButtonStyle())
            .disabled(selected.isEmpty || working)
        }
    }
}

/// Renaming a group, its description and photo (admins).
struct EditGroupSheet: View {
    let app: AppModel
    let chat: Chat
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var about = ""
    @State private var photo: Data?

    var body: some View {
        SheetScaffold(title: "Edit group", onClose: { dismiss() }) {
            Button {
                let panel = NSOpenPanel()
                panel.allowedContentTypes = [.image]
                panel.begin { response in
                    guard response == .OK, let url = panel.url, let file = try? MediaPrep.prepare(url: url) else { return }
                    Task { @MainActor in photo = file.thumbnail }
                }
            } label: {
                Avatar(id: chat.id, name: name, image: photo ?? chat.avatar, size: 80, isGroup: true)
                    .overlay(alignment: .bottomTrailing) {
                        Image(systemName: "camera.fill")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundStyle(.white)
                            .frame(width: 26, height: 26)
                            .background(Circle().fill(Palette.accent))
                    }
            }
            .buttonStyle(PressableStyle())
            TextField("Group name", text: $name)
                .textFieldStyle(.roundedBorder)
            TextField("Description", text: $about, axis: .vertical)
                .textFieldStyle(.roundedBorder)
                .lineLimit(2 ... 5)
            Spacer()
            Button("Save") {
                app.perform { try app.core.updateGroup(chat.id, name: name, description: about, avatar: photo) }
                dismiss()
            }
            .buttonStyle(PrimaryButtonStyle())
            .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
            .keyboardShortcut(.defaultAction)
        }
        .onAppear {
            name = chat.title
            about = chat.description
        }
    }
}

// MARK: - Shared pieces

struct SheetScaffold<Content: View>: View {
    let title: String
    let onClose: () -> Void
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Text(title).font(.system(size: 18, weight: .bold))
                Spacer()
                Button {
                    onClose()
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(.secondary)
                        .frame(width: 26, height: 26)
                        .background(Circle().fill(Palette.hairline))
                }
                .buttonStyle(PressableStyle())
                .keyboardShortcut(.cancelAction)
            }
            content()
        }
        .padding(22)
        .frame(width: 440, height: 560)
    }
}

struct SearchField: View {
    @Binding var text: String
    let placeholder: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField(placeholder, text: $text).textFieldStyle(.plain)
        }
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Palette.hairline))
    }
}

struct PersonRow: View {
    let contact: Contact

    var body: some View {
        HStack(spacing: 10) {
            Avatar(id: contact.id, name: contact.displayName, image: contact.avatar, size: 36)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 4) {
                    Text(contact.displayName).font(.system(size: 13.5, weight: .medium))
                    VerifiedBadge(contact.verification, compact: true)
                }
                Text(contact.verification.isVerified ? "Verified" : "Not verified")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(.vertical, 3)
    }
}
