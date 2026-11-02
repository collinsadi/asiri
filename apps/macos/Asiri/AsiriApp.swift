import AppKit
import AsiriBridge
import AsiriFeatures
import SwiftUI

@main
struct AsiriApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    private var launcher: Launcher { .shared }

    var body: some Scene {
        Window("Asiri", id: WindowID.main) {
            MainWindow(launcher: launcher)
        }
        .defaultSize(width: 1180, height: 760)
        .windowToolbarStyle(.unifiedCompact(showsTitle: false))
        .commands { AsiriCommands(app: launcher.app) }

        Window("Call", id: WindowID.call) {
            CallWindow(launcher: launcher)
        }
        .defaultSize(width: 900, height: 620)

        Settings {
            SettingsWindow(launcher: launcher)
        }

        MenuBarExtra {
            MenuBarContent(launcher: launcher)
        } label: {
            MenuBarLabel(launcher: launcher)
        }
        .menuBarExtraStyle(.window)
    }
}

/// App lifecycle (§6.7): closing the window keeps Asiri in the menu bar, the Dock icon brings
/// the window back, and quitting closes the connection cleanly.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { Launcher.shared.showMainWindow() }
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        let launcher = Launcher.shared
        guard let app = launcher.app else { return .terminateNow }
        app.hangUp()
        launcher.calls.end()
        guard let core = app.core as? LiveCore else { return .terminateNow }
        Task { @MainActor in
            // Give the session up to two seconds to close, then quit regardless.
            await withTaskGroup(of: Void.self) { group in
                group.addTask { await core.disconnect() }
                group.addTask { try? await Task.sleep(for: .seconds(2)) }
                await group.next()
                group.cancelAll()
            }
            sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}
