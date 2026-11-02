import AppKit
import AsiriDesign
import AsiriDomain
import SwiftUI

/// Menu-bar commands and keyboard shortcuts (§13.4).
public struct AsiriCommands: Commands {
    let app: AppModel

    public init(app: AppModel) {
        self.app = app
    }

    public var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Chat") { app.sheet = .newChat }
                .keyboardShortcut("n")
            Button("New Group") { app.sheet = .newGroup }
                .keyboardShortcut("n", modifiers: [.command, .shift])
            Button("Add a Friend in Person…") { app.sheet = .addFriend }
        }
        CommandMenu("Chat") {
            Button("Quick Switcher and Search") { app.quickSwitcherOpen = true }
                .keyboardShortcut("k")
            Divider()
            Button("Next Chat") { app.selectNext(true) }
                .keyboardShortcut(.tab, modifiers: [.control])
            Button("Previous Chat") { app.selectNext(false) }
                .keyboardShortcut(.tab, modifiers: [.control, .shift])
            Divider()
            Button(app.showDetails ? "Hide Details" : "Show Details") {
                withAnimation(Motion.standard) { app.showDetails.toggle() }
            }
            .keyboardShortcut("i", modifiers: [.command, .option])
            Divider()
            Button("Lock Asiri") { app.lock() }
                .keyboardShortcut("l")
                .disabled(!app.prefs.appLockEnabled)
        }
    }
}

/// The menu bar extra (§6.7): connection, unread, recent chats.
public struct MenuBarView: View {
    let app: AppModel
    let openMain: () -> Void

    public init(app: AppModel, openMain: @escaping () -> Void) {
        self.app = app
        self.openMain = openMain
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                AsiriMark(size: 22)
                Text("Asiri").font(.system(size: 13, weight: .bold))
                Spacer()
                StatusDot(app.connection)
                Text(app.connection.label).font(.system(size: 11)).foregroundStyle(.secondary)
            }
            if app.phase == .ready {
                let recent = Array(app.chats.filter { !$0.isArchived }.prefix(5))
                if recent.isEmpty {
                    Text("No chats yet").font(.system(size: 12)).foregroundStyle(.secondary)
                } else {
                    ForEach(recent) { chat in
                        Button {
                            app.select(chat.id)
                            openMain()
                        } label: {
                            HStack(spacing: 8) {
                                Avatar(id: chat.peer ?? chat.id, name: chat.title, image: chat.avatar, size: 24, isGroup: chat.kind == .group)
                                Text(chat.title).font(.system(size: 12.5, weight: .medium)).lineLimit(1)
                                Spacer()
                                if chat.unread > 0 { UnreadBadge(count: chat.unread, muted: chat.isMuted) }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            Divider()
            HStack {
                Button("Open Asiri", action: openMain)
                Spacer()
                if app.prefs.appLockEnabled {
                    Button("Lock") { app.lock() }
                }
                Button("Quit") { NSApp.terminate(nil) }
            }
            .font(.system(size: 12))
        }
        .padding(12)
        .frame(width: 280)
    }
}
