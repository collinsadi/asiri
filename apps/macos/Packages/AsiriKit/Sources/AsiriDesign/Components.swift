import AppKit
import AsiriDomain
import SwiftUI

// MARK: - Avatar

/// A round photo, or initials on a gradient picked from the ID.
public struct Avatar: View {
    let id: Data
    let name: String
    let image: Data?
    let size: CGFloat
    let isGroup: Bool

    public init(id: Data, name: String, image: Data? = nil, size: CGFloat = Metrics.rowAvatar, isGroup: Bool = false) {
        self.id = id
        self.name = name
        self.image = image
        self.size = size
        self.isGroup = isGroup
    }

    public var body: some View {
        ZStack {
            if let image, let nsImage = NSImage(data: image) {
                Image(nsImage: nsImage)
                    .resizable()
                    .scaledToFill()
            } else {
                let (top, bottom) = Palette.avatarGradients[Palette.index(for: id, count: Palette.avatarGradients.count)]
                LinearGradient(colors: [top, bottom], startPoint: .topLeading, endPoint: .bottomTrailing)
                if isGroup && initials.isEmpty {
                    Image(systemName: "person.3.fill")
                        .font(.system(size: size * 0.34, weight: .semibold))
                        .foregroundStyle(.white)
                } else {
                    Text(initials)
                        .font(.system(size: size * 0.38, weight: .semibold, design: .rounded))
                        .foregroundStyle(.white)
                        .minimumScaleFactor(0.5)
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .accessibilityHidden(true)
    }

    private var initials: String {
        let words = name.split(whereSeparator: { $0.isWhitespace }).prefix(2)
        return words.compactMap { $0.first.map(String.init) }.joined().uppercased()
    }
}

// MARK: - Badges

/// The unread pill: accent normally, gray when the chat is muted.
public struct UnreadBadge: View {
    let count: Int
    let muted: Bool

    public init(count: Int, muted: Bool = false) {
        self.count = count
        self.muted = muted
    }

    public var body: some View {
        Text(count > 999 ? "999+" : "\(count)")
            .font(.system(size: 11.5, weight: .bold, design: .rounded))
            .monospacedDigit()
            .foregroundStyle(.white)
            .padding(.horizontal, 6)
            .frame(minWidth: 20, minHeight: 20)
            .background(Capsule().fill(muted ? Color.gray.opacity(0.55) : Palette.accent))
            .contentTransition(.numericText(value: Double(count)))
            .accessibilityLabel("\(count) unread")
    }
}

/// "Verified" in amber, with the seal symbol (never color alone, §13.9).
public struct VerifiedBadge: View {
    let verification: Verification
    let compact: Bool

    public init(_ verification: Verification, compact: Bool = false) {
        self.verification = verification
        self.compact = compact
    }

    public var body: some View {
        switch verification {
        case .inPerson, .remote:
            Image(systemName: "checkmark.seal.fill")
                .font(.system(size: compact ? 11 : 13, weight: .semibold))
                .foregroundStyle(Palette.amber)
                .accessibilityLabel(verification == .inPerson ? "Verified in person" : "Verified")
        case .changed:
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: compact ? 11 : 13, weight: .semibold))
                .foregroundStyle(Palette.danger)
                .accessibilityLabel("Security key changed")
        case .unverified:
            EmptyView()
        }
    }
}

/// Delivery ticks (§13.5): clock, one tick, two ticks, two accent ticks, red mark.
public struct StatusTicks: View {
    let status: DeliveryStatus
    let tint: Color
    let readTint: Color

    public init(_ status: DeliveryStatus, tint: Color, readTint: Color) {
        self.status = status
        self.tint = tint
        self.readTint = readTint
    }

    public var body: some View {
        Group {
            switch status {
            case .pending:
                Image(systemName: "clock")
                    .font(.system(size: 9.5, weight: .semibold))
                    .foregroundStyle(tint)
            case .sent:
                Image(systemName: "checkmark")
                    .font(.system(size: 9.5, weight: .bold))
                    .foregroundStyle(tint)
            case .delivered, .read:
                DoubleTick()
                    .stroke(status == .read ? readTint : tint, style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
                    .frame(width: 15, height: 9)
            case .failed:
                Image(systemName: "exclamationmark.circle.fill")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(Palette.danger)
            case .received, .seen:
                EmptyView()
            }
        }
        .transition(.scale(scale: 0.4).combined(with: .opacity))
        .animation(Motion.snappy, value: status)
        .accessibilityLabel(Self.label(status))
    }

    public static func label(_ status: DeliveryStatus) -> String {
        switch status {
        case .pending: "Sending"
        case .sent: "Sent"
        case .delivered: "Delivered"
        case .read: "Read"
        case .failed: "Not sent"
        case .received, .seen: ""
        }
    }
}

/// Two overlapping check marks.
struct DoubleTick: Shape {
    func path(in r: CGRect) -> Path {
        var p = Path()
        let h = r.height
        // first tick
        p.move(to: CGPoint(x: r.minX, y: r.minY + h * 0.55))
        p.addLine(to: CGPoint(x: r.minX + h * 0.38, y: r.maxY))
        p.addLine(to: CGPoint(x: r.minX + h * 1.05, y: r.minY))
        // second tick
        p.move(to: CGPoint(x: r.minX + h * 0.62, y: r.maxY - h * 0.08))
        p.addLine(to: CGPoint(x: r.minX + h * 0.72, y: r.maxY))
        p.addLine(to: CGPoint(x: r.maxX, y: r.minY))
        return p
    }
}

// MARK: - Pills and chips

/// A centered translucent pill: date separators and system lines.
public struct CenterPill: View {
    let text: String
    let icon: String?

    public init(_ text: String, icon: String? = nil) {
        self.text = text
        self.icon = icon
    }

    public var body: some View {
        HStack(spacing: 5) {
            if let icon {
                Image(systemName: icon).font(.system(size: 10.5, weight: .semibold))
            }
            Text(text)
                .font(Typo.caption)
                .multilineTextAlignment(.center)
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .background(Capsule().fill(Palette.pill))
        .background(.ultraThinMaterial, in: Capsule())
    }
}

/// A typing indicator: three dots that rise in turn.
public struct TypingDots: View {
    let color: Color
    @State private var phase = 0.0

    public init(color: Color = Palette.accent) {
        self.color = color
    }

    public var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            HStack(spacing: 3) {
                ForEach(0 ..< 3, id: \.self) { i in
                    let wave = sin((t * 6) - Double(i) * 0.9)
                    Circle()
                        .fill(color)
                        .frame(width: 5, height: 5)
                        .offset(y: -2.5 * max(0, wave))
                        .opacity(0.45 + 0.55 * max(0, wave))
                }
            }
        }
        .frame(height: 10)
        .accessibilityLabel("typing")
    }
}

/// A pulsing dot for connection state.
public struct StatusDot: View {
    let status: ConnectionStatus

    public init(_ status: ConnectionStatus) {
        self.status = status
    }

    public var body: some View {
        Circle()
            .fill(color)
            .frame(width: 8, height: 8)
            .overlay {
                if !status.isOnline && status != .notRegistered {
                    Circle()
                        .stroke(color.opacity(0.6), lineWidth: 2)
                        .phaseAnimator([false, true]) { view, grow in
                            view.scaleEffect(grow ? 2.2 : 1).opacity(grow ? 0 : 1)
                        } animation: { _ in .easeOut(duration: 1.2) }
                }
            }
            .accessibilityLabel(status.label)
    }

    private var color: Color {
        switch status {
        case .connected: Palette.success
        case .syncing: Palette.accent
        case .connecting, .startingTor: Palette.amber
        case .offline, .notRegistered: Color.gray
        }
    }
}

// MARK: - Buttons

/// A round icon button used in headers and the composer.
public struct CircleIconButton: View {
    let systemName: String
    let size: CGFloat
    let filled: Bool
    let action: () -> Void
    @State private var hovering = false

    public init(_ systemName: String, size: CGFloat = 30, filled: Bool = false, action: @escaping () -> Void) {
        self.systemName = systemName
        self.size = size
        self.filled = filled
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: size * 0.46, weight: .semibold))
                .foregroundStyle(filled ? Color.white : (hovering ? Palette.accent : Color.secondary))
                .frame(width: size, height: size)
                .background {
                    Circle().fill(filled ? AnyShapeStyle(Palette.accent) : AnyShapeStyle(hovering ? Palette.accent.opacity(0.12) : Color.clear))
                }
                .contentShape(Circle())
        }
        .buttonStyle(PressableStyle())
        .onHover { hovering = $0 }
        .animation(Motion.quick, value: hovering)
    }
}

/// Scales down slightly while pressed.
public struct PressableStyle: ButtonStyle {
    public init() {}

    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.9 : 1)
            .animation(Motion.snappy, value: configuration.isPressed)
    }
}

/// The big call-to-action used in onboarding and pairing.
public struct PrimaryButtonStyle: ButtonStyle {
    let tint: Color

    public init(tint: Color = Palette.accent) {
        self.tint = tint
    }

    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(.white)
            .padding(.horizontal, 22)
            .frame(minHeight: 38)
            .background(
                RoundedRectangle(cornerRadius: 11, style: .continuous)
                    .fill(LinearGradient(colors: [tint.opacity(0.92), tint], startPoint: .top, endPoint: .bottom))
                    .shadow(color: tint.opacity(configuration.isPressed ? 0.15 : 0.35), radius: configuration.isPressed ? 3 : 10, y: configuration.isPressed ? 1 : 4)
            )
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(Motion.snappy, value: configuration.isPressed)
    }
}

/// A quiet secondary button.
public struct SecondaryButtonStyle: ButtonStyle {
    public init() {}

    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .medium))
            .foregroundStyle(Palette.accent)
            .padding(.horizontal, 18)
            .frame(minHeight: 36)
            .background(
                RoundedRectangle(cornerRadius: 11, style: .continuous)
                    .fill(Palette.accent.opacity(configuration.isPressed ? 0.18 : 0.1))
            )
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(Motion.snappy, value: configuration.isPressed)
    }
}

// MARK: - Emoji verification

/// The six (or four) emoji both people compare, each with its name.
public struct EmojiSequence: View {
    let emoji: [EmojiWord]
    let size: CGFloat
    @State private var shown = false

    public init(_ emoji: [EmojiWord], size: CGFloat = 40) {
        self.emoji = emoji
        self.size = size
    }

    public var body: some View {
        HStack(spacing: size * 0.35) {
            ForEach(Array(emoji.enumerated()), id: \.offset) { index, e in
                VStack(spacing: 6) {
                    Text(e.glyph)
                        .font(.system(size: size))
                    Text(e.name)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                }
                .scaleEffect(shown ? 1 : 0.3)
                .opacity(shown ? 1 : 0)
                .animation(Motion.bouncy.delay(Double(index) * 0.07), value: shown)
                .accessibilityElement(children: .combine)
            }
        }
        .onAppear { shown = true }
    }
}
