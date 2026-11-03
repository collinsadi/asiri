// Draws the picture behind the window Asiri's disk image opens: the app on the left, the
// Applications folder it's dragged onto on the right, and a path of dots between them.
// tools/release/make-dmg.sh runs it with the window's layout, so the drawing lines up with the
// icons Finder places on top:
//
//   swift tools/design/make-dmg-background.swift --out DIR --version 1.2.0 --size 660x420 \
//       --bleed 20 --icon-size 128 --text-size 13 --app 170,214 --applications 490,214
//
// Writes DIR/background.png and DIR/background@2x.png. Coordinates are points from the
// top-left corner of the window's content, as Finder places icons; --bleed extends the
// picture below that, for title bars shorter than the one make-dmg.sh allows for.

import AppKit

// MARK: - Layout

struct Layout {
    var size = CGSize.zero
    var iconSize: CGFloat = 0
    var textSize: CGFloat = 0
    var app = CGPoint.zero
    var applications = CGPoint.zero
    var bleed: CGFloat = 0
    var version = ""
    var out = URL(fileURLWithPath: ".")
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("make-dmg-background: \(message)\n".utf8))
    exit(2)
}

func parse(_ arguments: [String]) -> Layout {
    func point(_ text: String) -> CGPoint? {
        let parts = text.split(separator: ",").compactMap { Double($0) }
        return parts.count == 2 ? CGPoint(x: parts[0], y: parts[1]) : nil
    }
    var layout = Layout()
    var seen = Set<String>()
    var rest = arguments[...]
    while let flag = rest.popFirst() {
        guard let value = rest.popFirst() else { fail("\(flag) needs a value") }
        seen.insert(flag)
        switch flag {
        case "--out": layout.out = URL(fileURLWithPath: value, isDirectory: true)
        case "--version": layout.version = value
        case "--size":
            let parts = value.split(separator: "x").compactMap { Double($0) }
            guard parts.count == 2 else { fail("--size looks like 660x420") }
            layout.size = CGSize(width: parts[0], height: parts[1])
        case "--bleed": layout.bleed = CGFloat(Double(value) ?? 0)
        case "--icon-size": layout.iconSize = CGFloat(Double(value) ?? 0)
        case "--text-size": layout.textSize = CGFloat(Double(value) ?? 0)
        case "--app": layout.app = point(value) ?? { fail("--app looks like 170,214") }()
        case "--applications": layout.applications = point(value) ?? { fail("--applications looks like 490,214") }()
        default: fail("unknown option \(flag)")
        }
    }
    let required = ["--out", "--version", "--size", "--icon-size", "--text-size", "--app", "--applications"]
    let missing = required.filter { !seen.contains($0) }
    if !missing.isEmpty { fail("missing \(missing.joined(separator: ", "))") }
    return layout
}

// MARK: - Palette (AsiriDesign's Palette and the app icon)

let sRGB = CGColorSpace(name: CGColorSpace.sRGB)!

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

func nsColor(_ hex: UInt32, _ alpha: CGFloat = 1) -> NSColor { NSColor(cgColor: color(hex, alpha))! }

enum Ink {
    static let nightTop: UInt32 = 0x1C1945
    static let nightBottom: UInt32 = 0x0C0A20
    static let indigo: UInt32 = 0x5A51F2      // the icon's top
    static let violet: UInt32 = 0x7C3AED
    static let lavender: UInt32 = 0xA9A4FF    // the icon's glow
    static let mist: UInt32 = 0xC9C6FF
    // Finder draws the icons' names itself, over the picture: in black on macOS 15 and 27,
    // whatever the appearance or the window's color, though other versions may draw white.
    // This indigo is as far from one as from the other (about 4.5:1 against both), so the
    // plates under the names keep them readable either way.
    static let plate: UInt32 = 0x6762EE
}

// MARK: - Drawing

/// SplitMix64: the same deterministic scatter as ChatWallpaper.
func random(_ seed: UInt64) -> UInt64 {
    var z = seed &+ 0x9E37_79B9_7F4A_7C15
    z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
    z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
    return z ^ (z >> 31)
}

func radialGlow(_ cg: CGContext, at center: CGPoint, radius: CGFloat, _ hex: UInt32, _ alpha: CGFloat) {
    let gradient = CGGradient(colorsSpace: sRGB,
                              colors: [color(hex, alpha), color(hex, alpha * 0.42), color(hex, 0)] as CFArray,
                              locations: [0, 0.45, 1])!
    cg.drawRadialGradient(gradient, startCenter: center, startRadius: 0, endCenter: center, endRadius: radius, options: [])
}

/// The chat wallpaper's glyphs, scattered the same way, very faint.
func wallpaper(_ cg: CGContext, _ size: CGSize) {
    let glyphs = [
        "lock.fill", "key.fill", "moon.stars.fill", "sparkles", "bubble.left.fill", "heart.fill",
        "leaf.fill", "star.fill", "paperplane.fill", "camera.fill", "music.note", "cloud.fill",
        "shield.lefthalf.filled", "envelope.fill", "bolt.fill", "sun.max.fill",
    ]
    let configuration = NSImage.SymbolConfiguration(pointSize: 13, weight: .regular)
        .applying(NSImage.SymbolConfiguration(paletteColors: [nsColor(Ink.lavender)]))
    let symbols = glyphs.compactMap {
        NSImage(systemSymbolName: $0, accessibilityDescription: nil)?.withSymbolConfiguration(configuration)
    }
    guard !symbols.isEmpty else { return }
    let cell: CGFloat = 66
    for row in 0 ..< Int(size.height / cell) + 2 {
        for col in 0 ..< Int(size.width / cell) + 2 {
            let r = random(UInt64(truncatingIfNeeded: row &* 73_856_093 ^ col &* 19_349_663))
            let jitterX = (CGFloat(r & 0xFF) / 255 - 0.5) * cell * 0.5
            let jitterY = (CGFloat((r >> 8) & 0xFF) / 255 - 0.5) * cell * 0.5
            let offset: CGFloat = row % 2 == 0 ? 0 : cell / 2
            let angle = CGFloat(Double((r >> 16) & 0x3F) - 32) * .pi / 180
            let scale = 0.85 + CGFloat((r >> 24) & 0x3F) / 63 * 0.55
            let symbol = symbols[Int((r >> 32) % UInt64(symbols.count))]
            cg.saveGState()
            cg.translateBy(x: CGFloat(col) * cell + offset + jitterX, y: CGFloat(row) * cell + jitterY)
            cg.rotate(by: angle)
            cg.scaleBy(x: scale, y: scale)
            let s = symbol.size
            symbol.draw(in: CGRect(x: -s.width / 2, y: -s.height / 2, width: s.width, height: s.height),
                        from: .zero, operation: .sourceOver, fraction: 0.055, respectFlipped: true, hints: nil)
            cg.restoreGState()
        }
    }
}

/// Dots that grow and brighten toward the Applications folder, then a chevron: the app
/// icon's three "typing" dots, set in motion.
func path(_ cg: CGContext, from start: CGFloat, to end: CGFloat, y: CGFloat) {
    let count = 5
    let chevron: CGFloat = 13
    let span = end - start - chevron - 10
    for i in 0 ..< count {
        let t = CGFloat(i) / CGFloat(count - 1)
        let x = start + span * t
        let radius = 2.2 + 1.6 * t
        cg.setFillColor(color(Ink.mist, 0.22 + 0.6 * t))
        cg.fillEllipse(in: CGRect(x: x - radius, y: y - radius, width: radius * 2, height: radius * 2))
    }
    cg.saveGState()
    cg.setShadow(offset: .zero, blur: 10, color: color(Ink.lavender, 0.9))
    cg.setStrokeColor(color(0xFFFFFF, 0.95))
    cg.setLineWidth(3.2)
    cg.setLineCap(.round)
    cg.setLineJoin(.round)
    cg.move(to: CGPoint(x: end - chevron, y: y - chevron))
    cg.addLine(to: CGPoint(x: end, y: y))
    cg.addLine(to: CGPoint(x: end - chevron, y: y + chevron))
    cg.strokePath()
    cg.restoreGState()
}

/// A dashed outline around the Applications folder: where the app goes.
func target(_ cg: CGContext, around center: CGPoint, size: CGFloat) {
    let rect = CGRect(x: center.x - size / 2, y: center.y - size / 2, width: size, height: size)
    let shape = CGPath(roundedRect: rect, cornerWidth: size * 0.24, cornerHeight: size * 0.24, transform: nil)
    cg.saveGState()
    cg.addPath(shape)
    cg.setFillColor(color(Ink.lavender, 0.045))
    cg.fillPath()
    cg.addPath(shape)
    cg.setStrokeColor(color(Ink.lavender, 0.34))
    cg.setLineWidth(1.4)
    cg.setLineDash(phase: 0, lengths: [5, 5])
    cg.strokePath()
    cg.restoreGState()
}

/// The plate behind an icon's name (see Ink.plate).
func plate(_ cg: CGContext, under center: CGPoint, name: String, layout: Layout) {
    let font = NSFont.systemFont(ofSize: layout.textSize)
    let width = (name as NSString).size(withAttributes: [.font: font]).width + layout.textSize * 1.5
    let height = layout.textSize + 9
    // Finder sets the name about 8 points under the icon's frame.
    let top = center.y + layout.iconSize / 2 + 8
    let rect = CGRect(x: center.x - width / 2, y: top, width: width, height: height)
    let shape = CGPath(roundedRect: rect, cornerWidth: height / 2, cornerHeight: height / 2, transform: nil)
    cg.saveGState()
    cg.setShadow(offset: CGSize(width: 0, height: 2), blur: 8, color: color(0x05041A, 0.55))
    cg.addPath(shape)
    cg.setFillColor(color(Ink.plate))
    cg.fillPath()
    cg.restoreGState()
    cg.saveGState()
    cg.addPath(shape)
    cg.setStrokeColor(color(0xFFFFFF, 0.16))
    cg.setLineWidth(1)
    cg.strokePath()
    cg.restoreGState()
}

func text(_ string: String, in rect: CGRect, font: NSFont, _ hex: UInt32, _ alpha: CGFloat, kern: CGFloat = 0) {
    let style = NSMutableParagraphStyle()
    style.alignment = .center
    NSAttributedString(string: string, attributes: [
        .font: font, .foregroundColor: nsColor(hex, alpha), .kern: kern, .paragraphStyle: style,
    ]).draw(in: rect)
}

/// Fine grain, so the dark gradients don't band.
func grain(_ cg: CGContext, pixels: (Int, Int), size: CGSize) {
    let (w, h) = pixels
    var bytes = [UInt8](repeating: 255, count: w * h * 4)
    var state: UInt64 = 0xA5121
    for i in 0 ..< w * h {
        state = random(state)
        let v = UInt8(truncatingIfNeeded: state)
        bytes[i * 4] = v
        bytes[i * 4 + 1] = v
        bytes[i * 4 + 2] = v
    }
    let provider = CGDataProvider(data: Data(bytes) as CFData)!
    let noise = CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4, space: sRGB,
                        bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                        provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)!
    cg.saveGState()
    cg.setBlendMode(.overlay)
    cg.setAlpha(0.035)
    cg.draw(noise, in: CGRect(origin: .zero, size: size))
    cg.restoreGState()
}

func render(_ layout: Layout, scale: CGFloat) -> CGImage {
    let size = layout.size
    let canvas = CGSize(width: size.width, height: size.height + layout.bleed)
    let pixels = (Int(canvas.width * scale), Int(canvas.height * scale))
    let cg = CGContext(data: nil, width: pixels.0, height: pixels.1, bitsPerComponent: 8, bytesPerRow: 0, space: sRGB,
                       bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    cg.interpolationQuality = .high
    // Points from the top-left corner, like Finder's icon positions.
    cg.translateBy(x: 0, y: CGFloat(pixels.1))
    cg.scaleBy(x: scale, y: -scale)
    NSGraphicsContext.current = NSGraphicsContext(cgContext: cg, flipped: true)

    let night = CGGradient(colorsSpace: sRGB, colors: [color(Ink.nightTop), color(Ink.nightBottom)] as CFArray,
                           locations: [0, 1])!
    cg.drawLinearGradient(night, start: .zero, end: CGPoint(x: canvas.width * 0.3, y: canvas.height),
                          options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])

    radialGlow(cg, at: CGPoint(x: size.width / 2, y: -size.height * 0.15), radius: size.width * 0.55, Ink.indigo, 0.28)
    radialGlow(cg, at: CGPoint(x: size.width * 0.95, y: size.height * 1.05), radius: size.width * 0.45, Ink.violet, 0.16)
    wallpaper(cg, canvas)
    radialGlow(cg, at: layout.app, radius: layout.iconSize * 1.35, Ink.indigo, 0.55)
    radialGlow(cg, at: layout.applications, radius: layout.iconSize * 1.0, Ink.violet, 0.16)

    target(cg, around: layout.applications, size: layout.iconSize + 26)
    let gap = layout.iconSize / 2 + 26
    path(cg, from: layout.app.x + gap, to: layout.applications.x - gap - 8, y: layout.app.y)
    plate(cg, under: layout.app, name: "Asiri", layout: layout)
    plate(cg, under: layout.applications, name: "Applications", layout: layout)

    text("Drag Asiri to Applications", in: CGRect(x: 0, y: 34, width: size.width, height: 30),
         font: NSFont.systemFont(ofSize: 21, weight: .semibold), 0xFFFFFF, 0.96)
    text("Then open it from your Applications folder.", in: CGRect(x: 0, y: 64, width: size.width, height: 20),
         font: NSFont.systemFont(ofSize: 12.5, weight: .regular), Ink.mist, 0.68)
    text("VERSION \(layout.version)   ·   END-TO-END ENCRYPTED   ·   POST-QUANTUM   ·   ONION-ROUTED",
         in: CGRect(x: 0, y: size.height - 34, width: size.width, height: 16),
         font: NSFont.systemFont(ofSize: 9, weight: .semibold), Ink.mist, 0.42, kern: 1.1)

    // Darker corners draw the eye to the middle.
    let vignette = CGGradient(colorsSpace: sRGB, colors: [color(0x000000, 0), color(0x000000, 0.32)] as CFArray,
                              locations: [0.55, 1])!
    let center = CGPoint(x: size.width / 2, y: size.height / 2)
    cg.drawRadialGradient(vignette, startCenter: center, startRadius: 0, endCenter: center,
                          endRadius: hypot(size.width, size.height) / 2, options: [.drawsAfterEndLocation])
    grain(cg, pixels: pixels, size: canvas)
    return cg.makeImage()!
}

func save(_ image: CGImage, points: CGSize, to url: URL) {
    let rep = NSBitmapImageRep(cgImage: image)
    rep.size = points // so the @2x image says 144 dpi and Finder shows it at the same size
    guard let png = rep.representation(using: .png, properties: [:]) else { fail("couldn't encode \(url.path)") }
    do { try png.write(to: url) } catch { fail("couldn't write \(url.path): \(error.localizedDescription)") }
}

let layout = parse(Array(CommandLine.arguments.dropFirst()))
try? FileManager.default.createDirectory(at: layout.out, withIntermediateDirectories: true)
for (scale, name) in [(1.0, "background.png"), (2.0, "background@2x.png")] {
    let points = CGSize(width: layout.size.width, height: layout.size.height + layout.bleed)
    save(render(layout, scale: scale), points: points, to: layout.out.appendingPathComponent(name))
}
print("Drew \(layout.out.path)/background.png and background@2x.png")
