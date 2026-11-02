import AppKit
import AsiriDomain
import Foundation
import LocalAuthentication
import Network
import ServiceManagement
import UserNotifications

// MARK: - App lock

/// Touch ID or the Mac password (LocalAuthentication).
public enum AppLock {
    /// Whether this Mac can authenticate the user at all.
    public static var isAvailable: Bool {
        LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: nil)
    }

    /// Whether Touch ID specifically is set up.
    public static var hasBiometrics: Bool {
        LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
    }

    /// Asks for Touch ID or the password; returns whether the user is present.
    public static func authenticate(reason: String) async -> Bool {
        let context = LAContext()
        context.localizedCancelTitle = "Cancel"
        do {
            return try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
        } catch {
            return false
        }
    }
}

// MARK: - Sleep, wake and network

/// Tells the core when the Mac sleeps, wakes or changes network (architecture §6.7).
@MainActor
public final class SystemMonitor {
    private var observers: [NSObjectProtocol] = []
    private var sleptAt: Date?
    private let monitor = NWPathMonitor()
    private var lastPath: NWPath.Status?

    public var onWake: ((_ asleepSeconds: UInt64) -> Void)?
    public var onNetworkChange: (() -> Void)?

    public init() {}

    public func start() {
        let center = NSWorkspace.shared.notificationCenter
        observers.append(center.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.sleptAt = Date() }
        })
        observers.append(center.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                let asleep = self.sleptAt.map { UInt64(max(0, Date().timeIntervalSince($0))) } ?? 0
                self.sleptAt = nil
                self.onWake?(asleep)
            }
        })
        monitor.pathUpdateHandler = { [weak self] path in
            let status = path.status
            Task { @MainActor in
                guard let self else { return }
                if self.lastPath != nil, self.lastPath != status || status == .satisfied {
                    self.onNetworkChange?()
                }
                self.lastPath = status
            }
        }
        monitor.start(queue: DispatchQueue(label: "asiri.network-monitor"))
    }

    public func stop() {
        observers.forEach { NSWorkspace.shared.notificationCenter.removeObserver($0) }
        observers = []
        monitor.cancel()
    }
}

// MARK: - Notifications

/// How much a notification reveals (architecture §6.8). Private is the default: macOS keeps its
/// own copy of delivered notification text.
public enum NotificationLevel: String, CaseIterable, Sendable, Identifiable {
    case `private`
    case senderOnly
    case fullPreview

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .private: "Private"
        case .senderOnly: "Sender only"
        case .fullPreview: "Full preview"
        }
    }

    public var example: String {
        switch self {
        case .private: "“New message”"
        case .senderOnly: "“Ada sent a message”"
        case .fullPreview: "Sender and message text"
        }
    }
}

/// Local notifications only: Asiri never uses Apple's push service.
@MainActor
public final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    public static let callCategory = "asiri.call"
    public static let answerAction = "asiri.answer"
    public static let declineAction = "asiri.decline"

    public var onOpenChat: ((ChatID) -> Void)?
    public var onAnswer: ((CallID) -> Void)?
    public var onDecline: ((CallID) -> Void)?

    private var center: UNUserNotificationCenter? {
        // Tests and previews run without an app bundle, where the center would crash.
        Bundle.main.bundleIdentifier == nil ? nil : UNUserNotificationCenter.current()
    }

    override public init() {
        super.init()
    }

    public func setUp() {
        guard let center else { return }
        center.delegate = self
        let answer = UNNotificationAction(identifier: Self.answerAction, title: "Answer", options: [.foreground])
        let decline = UNNotificationAction(identifier: Self.declineAction, title: "Decline", options: [.destructive])
        center.setNotificationCategories([UNNotificationCategory(identifier: Self.callCategory, actions: [answer, decline], intentIdentifiers: [])])
        center.requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    public func notifyMessage(chat: ChatID, chatTitle: String, sender: String, text: String, level: NotificationLevel) {
        guard let center else { return }
        let content = UNMutableNotificationContent()
        switch level {
        case .private:
            content.title = "Asiri"
            content.body = "New message"
        case .senderOnly:
            content.title = "Asiri"
            content.body = "\(sender) sent a message"
        case .fullPreview:
            content.title = chatTitle
            content.body = chatTitle == sender ? text : "\(sender): \(text)"
        }
        content.sound = .default
        content.threadIdentifier = level == .private ? "asiri" : chat.base64EncodedString()
        content.userInfo = ["chat": chat.base64EncodedString()]
        center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
    }

    public func notifyCall(_ call: IncomingCall, caller: String, level: NotificationLevel) {
        guard let center else { return }
        let content = UNMutableNotificationContent()
        content.title = level == .private ? "Asiri" : caller
        content.body = call.isVideo ? "Incoming video call" : "Incoming call"
        content.sound = .default
        content.categoryIdentifier = Self.callCategory
        content.userInfo = ["call": call.callID.base64EncodedString()]
        center.add(UNNotificationRequest(identifier: "call-" + call.callID.base64EncodedString(), content: content, trigger: nil))
    }

    public func clearCall(_ call: CallID) {
        center?.removeDeliveredNotifications(withIdentifiers: ["call-" + call.base64EncodedString()])
    }

    public func clearAll() {
        center?.removeAllDeliveredNotifications()
    }

    public func setBadge(_ count: Int) {
        NSApp?.dockTile.badgeLabel = count > 0 ? "\(count)" : nil
    }

    nonisolated public func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse, withCompletionHandler completionHandler: @escaping () -> Void) {
        let info = response.notification.request.content.userInfo
        let action = response.actionIdentifier
        let chat = (info["chat"] as? String).flatMap { Data(base64Encoded: $0) }
        let call = (info["call"] as? String).flatMap { Data(base64Encoded: $0) }
        Task { @MainActor in
            if let call {
                if action == Self.declineAction { self.onDecline?(call) } else { self.onAnswer?(call) }
            } else if let chat {
                self.onOpenChat?(chat)
            }
            completionHandler()
        }
    }

    nonisolated public func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification, withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound])
    }
}

// MARK: - Login item and App Nap

public enum LaunchAtLogin {
    public static var isEnabled: Bool { SMAppService.mainApp.status == .enabled }

    public static func set(_ enabled: Bool) throws {
        if enabled { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
    }
}

/// Keeps the network loop alive while connected (opting out of App Nap only for that).
public final class NetworkActivity {
    private var token: NSObjectProtocol?

    public init() {}

    public func begin() {
        guard token == nil else { return }
        token = ProcessInfo.processInfo.beginActivity(options: [.userInitiatedAllowingIdleSystemSleep], reason: "Receiving messages")
    }

    public func end() {
        if let token { ProcessInfo.processInfo.endActivity(token) }
        token = nil
    }
}

// MARK: - Backups

public enum DataDirectory {
    /// The app container's data directory, excluded from Time Machine (architecture §6.6).
    public static func url() throws -> URL {
        let base = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        var dir = base.appendingPathComponent("Asiri", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? dir.setResourceValues(values)
        return dir
    }

    /// Deletes everything (panic wipe, after the key is destroyed).
    public static func erase() {
        guard let dir = try? url() else { return }
        try? FileManager.default.removeItem(at: dir)
    }
}
