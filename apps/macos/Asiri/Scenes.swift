import AppKit
import AsiriDesign
import AsiriDomain
import AsiriFeatures
import SwiftUI

/// The main window: the startup screens until the database is open, then the messenger.
struct MainWindow: View {
    let launcher: Launcher
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        Group {
            if let app = launcher.app {
                RootView(app: app)
            } else {
                StartupView(state: launcher.startup, retry: { launcher.open() }, startOver: { launcher.startOver() })
            }
        }
        .onAppear {
            launcher.windows = WindowActions(open: { openWindow(id: $0) }, dismiss: { dismissWindow(id: $0) })
            launcher.open()
        }
    }
}

/// The call window (§13.3). Closing it hangs up.
struct CallWindow: View {
    let launcher: Launcher

    var body: some View {
        if let app = launcher.app, let session = app.activeCall, let media = launcher.calls.media,
           launcher.calls.callID == session.callID {
            CallView(app: app, session: session, media: media)
                .id(session.callID)
                .onDisappear {
                    if app.activeCall?.callID == session.callID { app.hangUp() }
                }
        } else {
            ContentUnavailableView("No call right now", systemImage: "phone",
                                   description: Text("Calls you start or answer open here."))
                .frame(minWidth: 420, minHeight: 300)
        }
    }
}

/// ⌘, — available once the database is open.
struct SettingsWindow: View {
    let launcher: Launcher

    var body: some View {
        if let app = launcher.app {
            SettingsView(app: app)
        } else {
            ContentUnavailableView("Settings open with your chats", systemImage: "lock.fill",
                                   description: Text("Unlock Asiri first."))
                .frame(width: 420, height: 260)
        }
    }
}

/// The menu bar extra's panel (§6.7).
struct MenuBarContent: View {
    let launcher: Launcher

    var body: some View {
        if let app = launcher.app {
            MenuBarView(app: app, openMain: { launcher.showMainWindow() })
        } else {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 8) {
                    AsiriMark(size: 22)
                    Text("Asiri").font(.system(size: 13, weight: .bold))
                }
                Text(launcher.startup == .locked ? "Locked" : "Opening…")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                Divider()
                HStack {
                    Button("Open Asiri") { launcher.showMainWindow() }
                    Spacer()
                    Button("Quit") { NSApp.terminate(nil) }
                }
                .font(.system(size: 12))
            }
            .padding(12)
            .frame(width: 280)
        }
    }
}

/// The menu bar icon: the keyhole bubble, with a dot when something is unread. It's also the
/// one view that always exists, so it hands the app its window actions at launch.
struct MenuBarLabel: View {
    let launcher: Launcher
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        let unread = launcher.app?.totalUnread ?? 0
        Image(unread > 0 ? "MenuBarIconUnread" : "MenuBarIcon")
            .accessibilityLabel(unread > 0 ? "Asiri, \(unread) unread" : "Asiri")
            .onAppear {
                if launcher.windows == nil {
                    launcher.windows = WindowActions(open: { openWindow(id: $0) }, dismiss: { dismissWindow(id: $0) })
                }
            }
    }
}
