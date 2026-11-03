import AsiriDesign
import AsiriDomain
import SwiftUI

/// The main window: onboarding, the lock screen, or the three-pane messenger (§13.2).
public struct RootView: View {
    @Bindable var app: AppModel

    public init(app: AppModel) {
        self.app = app
    }

    public var body: some View {
        ZStack {
            switch app.phase {
            case .starting:
                LaunchView()
                    .transition(.opacity)
            case .onboarding:
                OnboardingView(app: app)
                    .transition(.opacity.combined(with: .scale(scale: 1.02)))
            case .locked:
                LockView(app: app)
                    .transition(.opacity)
            case .ready:
                if let update = app.updateAvailable, update.isRequired {
                    RequiredUpdateView(app: app, update: update)
                        .transition(.opacity)
                } else {
                    MainView(app: app)
                        .transition(.opacity)
                }
            }
        }
        // Above every phase: errors while setting up or unlocking need saying as much as later.
        .overlay(alignment: .top) {
            ToastView(toast: app.toast)
                .padding(.top, 10)
                .animation(Motion.bouncy, value: app.toast)
        }
        .overlay {
            if app.prefs.hideWhenInactive, !app.isActive, app.phase == .ready {
                PrivacyCover()
                    .transition(.opacity)
            }
        }
        .animation(Motion.standard, value: app.phase)
        .animation(Motion.quick, value: app.isActive)
        .environment(\.textScale, app.prefs.textScale)
        .frame(minWidth: 780, minHeight: 520)
    }
}

/// The three panes: chats, conversation, details.
struct MainView: View {
    @Bindable var app: AppModel
    @State private var columns = NavigationSplitViewVisibility.all

    var body: some View {
        NavigationSplitView(columnVisibility: $columns) {
            ChatListView(app: app)
                // Only the sidebar column can remove its toggle; the chat list is always there.
                .toolbar(removing: .sidebarToggle)
                // Outermost, so the split view sees the column's width.
                .navigationSplitViewColumnWidth(min: 260, ideal: Metrics.sidebarWidth, max: 420)
        } detail: {
            ZStack {
                if let id = app.selectedChatID, app.selectedChat != nil {
                    HStack(spacing: 0) {
                        ConversationView(model: app.conversation(for: id), app: app)
                            .id(id)
                            .transition(.opacity)
                        if app.showDetails {
                            Divider()
                            DetailsView(model: app.conversation(for: id), app: app)
                                .frame(width: 300)
                                .transition(.move(edge: .trailing).combined(with: .opacity))
                        }
                    }
                    .animation(Motion.standard, value: app.showDetails)
                } else {
                    EmptyConversationView(app: app)
                }
            }
        }
        .overlay(alignment: .top) {
            if let call = app.incomingCall {
                IncomingCallBanner(app: app, call: call)
                    .padding(.top, 12)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .animation(Motion.bouncy, value: app.incomingCall)
        .sheet(item: $app.sheet) { sheet in
            switch sheet {
            case .newChat: NewChatSheet(app: app)
            case .newGroup: NewGroupSheet(app: app)
            case .addFriend: AddFriendSheet(app: app)
            case let .addMembers(chat): AddMembersSheet(app: app, chatID: chat)
            case let .verify(user): VerifySheet(app: app, user: user)
            }
        }
        .sheet(isPresented: $app.quickSwitcherOpen) {
            QuickSwitcher(app: app)
        }
    }
}

/// Before the database opens.
struct LaunchView: View {
    var body: some View {
        ZStack {
            Palette.night.ignoresSafeArea()
            AsiriMark(size: 84)
                .phaseAnimator([false, true]) { mark, glow in
                    mark.shadow(color: Palette.indigoBright.opacity(glow ? 0.8 : 0.2), radius: glow ? 30 : 8)
                } animation: { _ in .easeInOut(duration: 1.4) }
        }
    }
}

/// Covers the window while Asiri isn't the frontmost app, when the user asked for that.
struct PrivacyCover: View {
    var body: some View {
        ZStack {
            Palette.night
            VStack(spacing: 14) {
                AsiriMark(size: 72)
                Text("Hidden while you're in another app")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(.white.opacity(0.7))
            }
        }
        .ignoresSafeArea()
    }
}

/// When no chat is selected.
struct EmptyConversationView: View {
    let app: AppModel

    var body: some View {
        ZStack {
            if app.prefs.showWallpaper { ChatWallpaper() } else { Palette.wallpaperTop }
            VStack(spacing: 16) {
                AsiriMark(size: 72)
                Text(app.chats.isEmpty ? "Add your first friend" : "Pick a chat")
                    .font(.system(size: 18, weight: .semibold))
                Text(app.chats.isEmpty
                    ? "Asiri connects people who meet in person. Open Asiri on your friend's Mac and scan each other's codes."
                    : "Only you and the people in a chat can read it.")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 340)
                if app.chats.isEmpty {
                    Button("Add a friend") { app.sheet = .addFriend }
                        .buttonStyle(PrimaryButtonStyle())
                }
            }
            .padding(32)
            .background(RoundedRectangle(cornerRadius: 24, style: .continuous).fill(.regularMaterial))
        }
    }
}

/// Slides in at the top; quiet unless something is wrong.
struct ToastView: View {
    let toast: Toast?

    var body: some View {
        if let toast {
            HStack(spacing: 8) {
                Image(systemName: icon(toast.style))
                    .foregroundStyle(color(toast.style))
                Text(toast.text)
                    .font(.system(size: 13, weight: .medium))
                    .lineLimit(3)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .background(Capsule().fill(.regularMaterial))
            .overlay(Capsule().stroke(Palette.hairline))
            .shadow(color: .black.opacity(0.15), radius: 12, y: 4)
            .transition(.move(edge: .top).combined(with: .opacity).combined(with: .scale(scale: 0.9)))
            .id(toast.id)
        }
    }

    private func icon(_ s: Toast.Style) -> String {
        switch s {
        case .info: "info.circle.fill"
        case .success: "checkmark.circle.fill"
        case .warning: "exclamationmark.shield.fill"
        case .error: "xmark.octagon.fill"
        }
    }

    private func color(_ s: Toast.Style) -> Color {
        switch s {
        case .info: Palette.accent
        case .success: Palette.success
        case .warning: Palette.amber
        case .error: Palette.danger
        }
    }
}

// MARK: - Text size

private struct TextScaleKey: EnvironmentKey {
    static let defaultValue: Double = 1
}

public extension EnvironmentValues {
    /// The in-app text size (§13.9).
    var textScale: Double {
        get { self[TextScaleKey.self] }
        set { self[TextScaleKey.self] = newValue }
    }
}
