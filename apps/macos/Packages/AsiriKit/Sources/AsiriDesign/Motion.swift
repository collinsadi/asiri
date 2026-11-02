import AppKit
import SwiftUI

/// Springs used everywhere (about 0.25 s, §13.7). They fall back to a short fade when the user
/// asks for reduced motion.
public enum Motion {
    private static var reduce: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    /// Bubbles, rows, sheets.
    public static var standard: Animation { reduce ? .easeOut(duration: 0.15) : .spring(response: 0.32, dampingFraction: 0.82) }
    /// Buttons and small state changes.
    public static var snappy: Animation { reduce ? .easeOut(duration: 0.1) : .spring(response: 0.22, dampingFraction: 0.78) }
    /// Playful: reactions, emoji, badges.
    public static var bouncy: Animation { reduce ? .easeOut(duration: 0.15) : .spring(response: 0.38, dampingFraction: 0.58) }
    /// Hover highlights.
    public static var quick: Animation { .easeOut(duration: reduce ? 0.05 : 0.14) }

    /// How a new message enters the timeline.
    public static func messageInsertion(outgoing: Bool) -> AnyTransition {
        if reduce { return .opacity }
        return .asymmetric(
            insertion: .scale(scale: 0.86, anchor: outgoing ? .bottomTrailing : .bottomLeading)
                .combined(with: .opacity)
                .combined(with: .offset(y: 14)),
            removal: .scale(scale: 0.9).combined(with: .opacity)
        )
    }
}

public extension View {
    /// Animates a change only when Reduce Motion is off.
    func motion<V: Equatable>(_ animation: Animation = Motion.standard, value: V) -> some View {
        self.animation(animation, value: value)
    }

    /// Keeps a symbol bouncing. Bounce can only repeat from macOS 15, so macOS 14 pulses instead.
    func repeatingBounce(speed: Double = 1) -> some View {
        modifier(RepeatingBounce(speed: speed))
    }
}

private struct RepeatingBounce: ViewModifier {
    let speed: Double

    func body(content: Content) -> some View {
        if #available(macOS 15, *) {
            content.symbolEffect(.bounce, options: .repeating.speed(speed))
        } else {
            content.symbolEffect(.pulse, options: .repeating.speed(speed))
        }
    }
}
