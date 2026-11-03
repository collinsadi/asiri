import SwiftUI

/// The conversation background: a soft gradient with a faint, hand-scattered pattern of SF
/// Symbols (locks, keys, moons, sparks). Deterministic, so it never shifts between launches, and
/// rasterized once.
public struct ChatWallpaper: View {
    private static let glyphs = [
        "lock.fill", "key.fill", "moon.stars.fill", "sparkles", "bubble.left.fill", "heart.fill",
        "leaf.fill", "star.fill", "paperplane.fill", "camera.fill", "music.note", "cloud.fill",
        "shield.lefthalf.filled", "envelope.fill", "bolt.fill", "sun.max.fill",
    ]

    public init() {}

    public var body: some View {
        ZStack {
            LinearGradient(colors: [Palette.wallpaperTop, Palette.wallpaperBottom], startPoint: .topLeading, endPoint: .bottomTrailing)
            Canvas { context, size in
                let cell: CGFloat = 78
                let cols = Int(size.width / cell) + 2
                let rows = Int(size.height / cell) + 2
                var symbols: [GraphicsContext.ResolvedImage] = []
                for name in Self.glyphs {
                    var image = context.resolve(Image(systemName: name))
                    image.shading = .color(Palette.wallpaperInk)
                    symbols.append(image)
                }
                for row in 0 ..< rows {
                    for col in 0 ..< cols {
                        let seed = UInt64(truncatingIfNeeded: row &* 73_856_093 ^ col &* 19_349_663)
                        let r = Self.random(seed)
                        let jitterX = (CGFloat(r & 0xFF) / 255 - 0.5) * cell * 0.5
                        let jitterY = (CGFloat((r >> 8) & 0xFF) / 255 - 0.5) * cell * 0.5
                        let offset: CGFloat = row % 2 == 0 ? 0 : cell / 2
                        let point = CGPoint(x: CGFloat(col) * cell + offset + jitterX, y: CGFloat(row) * cell + jitterY)
                        let angle = Angle.degrees(Double((r >> 16) & 0x3F) - 32)
                        let scale = 0.85 + CGFloat((r >> 24) & 0x3F) / 63 * 0.55
                        let symbol = symbols[Int((r >> 32) % UInt64(symbols.count))]
                        var c = context
                        c.translateBy(x: point.x, y: point.y)
                        c.rotate(by: angle)
                        c.scaleBy(x: scale * 1.15, y: scale * 1.15)
                        c.draw(symbol, at: .zero, anchor: .center)
                    }
                }
            }
            .drawingGroup()
        }
        .ignoresSafeArea()
        .accessibilityHidden(true)
    }

    /// SplitMix64: a cheap deterministic scatter.
    private static func random(_ seed: UInt64) -> UInt64 {
        var z = seed &+ 0x9E37_79B9_7F4A_7C15
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
}
