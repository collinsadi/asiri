import AsiriPlatform
import Foundation
import Observation

/// Display preferences that live outside the encrypted database because they're needed before
/// it is unlocked, or are purely cosmetic. Nothing here reveals who the user talks to.
@MainActor
@Observable
public final class Preferences {
    public static let shared = Preferences()

    private let defaults: UserDefaults

    public var notificationLevel: NotificationLevel {
        didSet { defaults.set(notificationLevel.rawValue, forKey: "notificationLevel") }
    }

    /// In-app text size (§13.9), 0.85…1.4.
    public var textScale: Double {
        didSet { defaults.set(textScale, forKey: "textScale") }
    }

    /// Touch ID or the Mac password is needed to open Asiri.
    public var appLockEnabled: Bool {
        didSet { defaults.set(appLockEnabled, forKey: "appLockEnabled") }
    }

    /// Lock after this many minutes in the background (0 = only when quitting).
    public var autoLockMinutes: Int {
        didSet { defaults.set(autoLockMinutes, forKey: "autoLockMinutes") }
    }

    /// Show the wallpaper pattern behind conversations.
    public var showWallpaper: Bool {
        didSet { defaults.set(showWallpaper, forKey: "showWallpaper") }
    }

    /// Hide the main window's contents when Asiri isn't the active app.
    public var hideWhenInactive: Bool {
        didSet { defaults.set(hideWhenInactive, forKey: "hideWhenInactive") }
    }

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        notificationLevel = NotificationLevel(rawValue: defaults.string(forKey: "notificationLevel") ?? "") ?? .private
        textScale = defaults.object(forKey: "textScale") as? Double ?? 1
        appLockEnabled = defaults.bool(forKey: "appLockEnabled")
        autoLockMinutes = defaults.object(forKey: "autoLockMinutes") as? Int ?? 15
        showWallpaper = defaults.object(forKey: "showWallpaper") as? Bool ?? true
        hideWhenInactive = defaults.bool(forKey: "hideWhenInactive")
    }

    /// An isolated instance for tests and previews.
    public static func ephemeral() -> Preferences {
        let suite = "asiri.preview.\(UUID().uuidString)"
        return Preferences(defaults: UserDefaults(suiteName: suite) ?? .standard)
    }
}
