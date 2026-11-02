import AppKit
import SwiftUI

// Asiri's visual language (architecture §13.8, docs/adr/0005): Night, Indigo and Amber on the
// system grays, SF Pro for the interface and SF Mono for safety numbers.

public extension Color {
    /// `#RRGGBB` or `#RRGGBBAA`.
    init(hex: UInt32, opacity: Double = 1) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: opacity
        )
    }

    /// A color that follows the system appearance.
    static func dynamic(light: UInt32, dark: UInt32, lightOpacity: Double = 1, darkOpacity: Double = 1) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            let hex = isDark ? dark : light
            return NSColor(
                srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: isDark ? darkOpacity : lightOpacity
            )
        })
    }
}

/// The palette. Everything in the UI takes its colors from here.
public enum Palette {
    public static let night = Color(hex: 0x14122B)
    public static let indigo = Color(hex: 0x4F46E5)
    public static let indigoBright = Color(hex: 0x6366F1)
    public static let violet = Color(hex: 0x7C3AED)
    public static let amber = Color(hex: 0xF5A524)
    public static let danger = Color(hex: 0xE5484D)
    public static let success = Color(hex: 0x30A46C)

    /// The accent used for selection, links and controls.
    public static let accent = Color.dynamic(light: 0x4F46E5, dark: 0x7B78FF)

    /// Outgoing bubbles: an indigo gradient with white text.
    public static let outgoingTop = Color.dynamic(light: 0x625BF6, dark: 0x5B55F2)
    public static let outgoingBottom = Color.dynamic(light: 0x4F46E5, dark: 0x4338CA)
    public static let outgoingText = Color.white
    public static let outgoingSecondary = Color.white.opacity(0.72)

    /// Incoming bubbles.
    public static let incoming = Color.dynamic(light: 0xFFFFFF, dark: 0x25233F)
    public static let incomingText = Color.dynamic(light: 0x1B1A2E, dark: 0xF1F0FF)
    public static let incomingSecondary = Color.dynamic(light: 0x8A88A3, dark: 0x9C9AB8)

    /// Conversation background (under the wallpaper pattern).
    public static let wallpaperTop = Color.dynamic(light: 0xE9E8FF, dark: 0x15132D)
    public static let wallpaperBottom = Color.dynamic(light: 0xF7F2FF, dark: 0x1D1A3B)
    public static let wallpaperInk = Color.dynamic(light: 0x4F46E5, dark: 0xA5A3FF, lightOpacity: 0.07, darkOpacity: 0.06)

    /// Panels and chrome.
    public static let sidebar = Color.dynamic(light: 0xF6F5FB, dark: 0x17152F)
    public static let surface = Color.dynamic(light: 0xFFFFFF, dark: 0x1E1C38)
    public static let elevated = Color.dynamic(light: 0xFFFFFF, dark: 0x2A2847)
    public static let hairline = Color.dynamic(light: 0x000000, dark: 0xFFFFFF, lightOpacity: 0.08, darkOpacity: 0.08)
    public static let selection = Color.dynamic(light: 0x4F46E5, dark: 0x5B55F2)
    public static let pill = Color.dynamic(light: 0x1B1A2E, dark: 0x000000, lightOpacity: 0.32, darkOpacity: 0.4)

    /// Avatar gradients, picked by ID so a person keeps their color everywhere.
    public static let avatarGradients: [(Color, Color)] = [
        (Color(hex: 0xFF8A65), Color(hex: 0xF4511E)),
        (Color(hex: 0xFFB74D), Color(hex: 0xF57C00)),
        (Color(hex: 0x81C784), Color(hex: 0x2E7D32)),
        (Color(hex: 0x4DD0E1), Color(hex: 0x00838F)),
        (Color(hex: 0x64B5F6), Color(hex: 0x1565C0)),
        (Color(hex: 0x9575CD), Color(hex: 0x5E35B1)),
        (Color(hex: 0xF06292), Color(hex: 0xC2185B)),
        (Color(hex: 0x7986CB), Color(hex: 0x3949AB)),
    ]

    /// Sender-name colors in groups (readable on incoming bubbles in both themes).
    public static let senderColors: [Color] = [
        .dynamic(light: 0xD84315, dark: 0xFF8A65),
        .dynamic(light: 0xEF6C00, dark: 0xFFB74D),
        .dynamic(light: 0x2E7D32, dark: 0x81C784),
        .dynamic(light: 0x00838F, dark: 0x4DD0E1),
        .dynamic(light: 0x1565C0, dark: 0x64B5F6),
        .dynamic(light: 0x5E35B1, dark: 0xB39DDB),
        .dynamic(light: 0xC2185B, dark: 0xF48FB1),
        .dynamic(light: 0x3949AB, dark: 0x9FA8DA),
    ]

    /// A stable index for an ID.
    public static func index(for id: Data, count: Int) -> Int {
        var h: UInt32 = 2_166_136_261
        for b in id {
            h = (h ^ UInt32(b)) &* 16_777_619
        }
        return Int(h % UInt32(max(count, 1)))
    }
}

/// Spacing, radii and sizes.
public enum Metrics {
    public static let bubbleRadius: CGFloat = 17
    public static let bubbleTightRadius: CGFloat = 6
    public static let bubbleMaxWidth: CGFloat = 520
    public static let bubblePaddingH: CGFloat = 12
    public static let bubblePaddingV: CGFloat = 7
    public static let rowAvatar: CGFloat = 46
    public static let composerMinHeight: CGFloat = 38
    public static let sidebarWidth: CGFloat = 320
}

/// Typography.
public enum Typo {
    public static let title = Font.system(size: 15, weight: .semibold)
    public static let rowTitle = Font.system(size: 13.5, weight: .semibold)
    public static let rowPreview = Font.system(size: 13)
    public static let message = Font.system(size: 14)
    public static let meta = Font.system(size: 11)
    public static let caption = Font.system(size: 11.5, weight: .medium)
    public static let mono = Font.system(size: 13, weight: .medium, design: .monospaced)
    public static let hero = Font.system(size: 30, weight: .bold, design: .rounded)
}
