import AppKit
import AsiriBridge
import AsiriDomain
import AsiriFeatures
import AsiriPlatform
import Foundation
import Observation

/// Opens the encrypted database and owns everything that lives as long as the app: the app
/// state, notifications, sleep and network monitoring, and the call window's media.
@MainActor
@Observable
final class Launcher {
    static let shared = Launcher()

    private(set) var startup: StartupState = .opening
    private(set) var app: AppModel?
    let calls = CallCoordinator()
    /// `--demo` (or ASIRI_DEMO=1) runs on PreviewCore: seeded chats, no server, nothing saved.
    let isDemo: Bool

    /// Window actions, captured from the first scene that appears.
    @ObservationIgnored var windows: WindowActions?

    private let notifier = Notifier()
    private let monitor = SystemMonitor()
    private let activity = NetworkActivity()
    @ObservationIgnored private var opening = false
    @ObservationIgnored private var monitoring = false
    @ObservationIgnored private var shownCall: CallID?
    @ObservationIgnored private var ringingCall: CallID?
    @ObservationIgnored private var activationObservers: [NSObjectProtocol] = []

    private init() {
        let process = ProcessInfo.processInfo
        isDemo = process.arguments.contains("--demo") || process.environment["ASIRI_DEMO"] == "1"
    }

    // MARK: Opening

    /// Unwraps the database key (Touch ID or the password when app lock is on) and opens the
    /// store off the main thread. Safe to call again after a cancel or a failure.
    func open() {
        guard app == nil, !opening else { return }
        opening = true
        startup = .opening
        if isDemo {
            opening = false
            attach(PreviewCore())
            return
        }
        Task {
            defer { opening = false }
            do {
                let core = try await Task.detached(priority: .userInitiated) { try Launcher.openCore() }.value
                attach(core)
            } catch KeyVault.VaultError.cancelled {
                startup = .locked
            } catch {
                startup = .failed(Self.describe(error))
            }
        }
    }

    nonisolated private static func openCore() throws -> LiveCore {
        if let level = ProcessInfo.processInfo.environment["ASIRI_LOG"], !level.isEmpty {
            LiveCore.enableLogging(level)
        }
        let key = try KeyVault().databaseKey(reason: "open your chats")
        let directory = try DataDirectory.url()
        return try LiveCore(dataDirectory: directory, databaseKey: key, releasePublicKey: Build.releasePublicKey, appBuild: Build.number)
    }

    private func attach(_ core: CoreService) {
        let app = AppModel(core: core, prefs: isDemo ? .ephemeral() : .shared)
        app.notifier = notifier
        app.onPanicWipe = { [weak self] in self?.restartAfterWipe() }
        notifier.onOpenChat = { [weak self] chat in
            guard let self, let app = self.app else { return }
            app.select(chat)
            self.showMainWindow()
        }
        notifier.onAnswer = { [weak self] call in
            guard let self, let app = self.app, let incoming = app.incomingCall, incoming.callID == call else { return }
            self.showMainWindow()
            Task { await app.answer(incoming) }
        }
        notifier.onDecline = { [weak self] call in
            guard let app = self?.app, let incoming = app.incomingCall, incoming.callID == call else { return }
            app.decline(incoming)
        }
        if !isDemo {
            notifier.setUp()
            startMonitoring()
        }
        observeActivation()
        self.app = app
        app.start()
        // Touch ID guards the launch only when the Secure Enclave wraps the database key. In
        // builds without it, and whenever a PIN is set, the lock screen comes first instead.
        if !isDemo, PINStore().hasPIN || (app.prefs.appLockEnabled && KeyVault().protection != .secureEnclave) {
            app.lock()
        }
        watch(app)
        react(to: app)
    }

    /// Sleep, wake and network changes go to whichever core is open.
    private func startMonitoring() {
        guard !monitoring else { return }
        monitoring = true
        monitor.onWake = { [weak self] seconds in _ = try? self?.app?.core.notifyWake(asleepSeconds: seconds) }
        monitor.onNetworkChange = { [weak self] in self?.app?.core.notifyNetworkChange() }
        monitor.start()
    }

    /// Auto-lock and "hide when inactive" follow whether Asiri is the frontmost app.
    private func observeActivation() {
        guard activationObservers.isEmpty else { return }
        let center = NotificationCenter.default
        activationObservers.append(center.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.app?.didResignActive() }
        })
        activationObservers.append(center.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.app?.didBecomeActive() }
        })
    }

    // MARK: Starting over

    /// After a panic wipe: a new key and an empty database, exactly like a fresh install.
    private func restartAfterWipe() {
        calls.end()
        windows?.dismiss(WindowID.call)
        app = nil
        shownCall = nil
        ringingCall = nil
        Preferences.shared.appLockEnabled = false
        open()
    }

    /// From the "couldn't open" screen: erase this Mac's copy and begin again.
    func startOver() {
        guard app == nil else { return }
        KeyVault().destroy()
        PINStore().clear()
        DataDirectory.erase()
        Preferences.shared.appLockEnabled = false
        open()
    }

    // MARK: Windows

    func showMainWindow() {
        NSApp.activate()
        windows?.open(WindowID.main)
    }

    /// Follows the app state that needs windows or the system: the call window, a ringing call
    /// when the main window is closed, and App Nap while connected.
    private func watch(_ app: AppModel) {
        withObservationTracking {
            _ = app.connection
            _ = app.activeCall
            _ = app.incomingCall
        } onChange: { [weak self, weak app] in
            Task { @MainActor in
                guard let self, let app, self.app === app else { return }
                self.react(to: app)
                self.watch(app)
            }
        }
    }

    private func react(to app: AppModel) {
        if app.connection.isOnline { activity.begin() } else { activity.end() }

        let call = app.activeCall
        if call?.callID != shownCall {
            shownCall = call?.callID
            if let call {
                calls.begin(call, app: app, demo: isDemo)
                NSApp.activate()
                windows?.open(WindowID.call)
            } else {
                calls.end()
                windows?.dismiss(WindowID.call)
            }
        }

        let ringing = app.incomingCall?.callID
        if let ringing, ringing != ringingCall {
            NSApp.requestUserAttention(.criticalRequest)
            showMainWindow()
        }
        ringingCall = ringing
    }

    private static func describe(_ error: Error) -> String {
        let text = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        return text.isEmpty ? "Something unexpected went wrong." : text
    }
}

/// The scenes' window actions, so the app can open and close windows from outside a view.
struct WindowActions {
    var open: (String) -> Void
    var dismiss: (String) -> Void
}

enum WindowID {
    static let main = "main"
    static let call = "call"
}

/// What this build is, from its Info.plist.
enum Build {
    static var number: UInt32 {
        UInt32(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "") ?? 0
    }

    /// The release-signing key (Ed25519, 32 bytes); `nil` in builds without one, which turns
    /// update checks off.
    static var releasePublicKey: Data? {
        guard let text = Bundle.main.object(forInfoDictionaryKey: "AsiriReleasePublicKey") as? String,
              let key = Data(base64Encoded: text.trimmingCharacters(in: .whitespaces)), key.count == 32
        else { return nil }
        return key
    }
}
