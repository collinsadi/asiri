import AppKit
import CoreImage
import CoreImage.CIFilterBuiltins
import SwiftUI

// MARK: - QR codes

/// Renders pairing codes as crisp QR images (CoreImage, nothing leaves the Mac).
public enum QRCode {
    /// A QR code image for `text`, scaled up without smoothing.
    public static func image(for text: String, scale: CGFloat = 12) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        let context = CIContext(options: [.useSoftwareRenderer: false])
        guard let cg = context.createCGImage(scaled, from: scaled.extent) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: scaled.extent.width, height: scaled.extent.height))
    }
}

/// A pairing code shown large, with Asiri's mark in the middle.
public struct QRCodeView: View {
    let code: String
    let size: CGFloat
    @State private var appeared = false

    public init(code: String, size: CGFloat = 240) {
        self.code = code
        self.size = size
    }

    public var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 22, style: .continuous)
                .fill(Color.white)
                .shadow(color: Palette.indigo.opacity(0.25), radius: 18, y: 8)
            if let image = QRCode.image(for: code) {
                Image(nsImage: image)
                    .interpolation(.none)
                    .resizable()
                    .scaledToFit()
                    .padding(size * 0.07)
                    .colorMultiply(Palette.night)
            }
            AsiriMark(size: size * 0.17)
                .padding(5)
                .background(Circle().fill(Color.white))
        }
        .frame(width: size, height: size)
        .scaleEffect(appeared ? 1 : 0.92)
        .opacity(appeared ? 1 : 0)
        .onAppear { withAnimation(Motion.bouncy) { appeared = true } }
        .accessibilityLabel("Pairing code")
    }
}

// MARK: - The mark

/// Asiri's mark, as on the app icon: a round speech bubble whose tail is a keyhole's slot, with
/// three "typing" dots cut through it, on deep indigo (§13.8).
public struct AsiriMark: View {
    let size: CGFloat

    public init(size: CGFloat = 64) {
        self.size = size
    }

    public var body: some View {
        ZStack {
            Circle()
                .fill(LinearGradient(colors: [Color(hex: 0x5A51F2), Color(hex: 0x3B32C4), Color(hex: 0x1F1873)],
                                     startPoint: .top, endPoint: .bottom))
            Circle()
                .fill(RadialGradient(colors: [Color(hex: 0xA9A4FF).opacity(0.5), .clear], center: .top,
                                     startRadius: 0, endRadius: size * 0.75))
            KeyholeBubble()
                .fill(LinearGradient(colors: [Color.white, Color(hex: 0xE3E1FF)], startPoint: .top, endPoint: .bottom),
                      style: FillStyle(eoFill: true))
                .shadow(color: Color(hex: 0x0B0830).opacity(0.35), radius: size * 0.03, y: size * 0.02)
                .frame(width: size * 0.47, height: size * 0.6)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// The bubble from the icon, fitted to the rect: a round head (the keyhole's), a slot that
/// flares down from it (the bubble's tail), and three dots cut out with even-odd filling.
/// Same proportions as tools/design/make-icon.py: head radius 238, slot half-widths 94 and 138,
/// foot at 602 below the head's top, foot corners 46, dots of 39 every 114.
public struct KeyholeBubble: Shape {
    public init() {}

    public func path(in rect: CGRect) -> Path {
        let width: CGFloat = 476, height: CGFloat = 602
        let s = min(rect.width / width, rect.height / height)
        let ox = rect.midX - width * s / 2
        let oy = rect.midY - height * s / 2
        func pt(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: ox + x * s, y: oy + y * s) }

        let r: CGFloat = 238, cx: CGFloat = 238, cy: CGFloat = 238
        let head = Path(ellipseIn: CGRect(x: ox, y: oy, width: 2 * r * s, height: 2 * r * s))

        // The slot starts inside the head, so the union is one solid shape.
        let top: CGFloat = 94, foot: CGFloat = 138, bottom: CGFloat = 602, corner: CGFloat = 46
        let dx = foot - top, dy = bottom - cy
        let len = (dx * dx + dy * dy).squareRoot()
        let ux = dx / len, uy = dy / len
        var slot = Path()
        slot.move(to: pt(cx - top, cy))
        slot.addLine(to: pt(cx + top, cy))
        slot.addLine(to: pt(cx + foot - ux * corner, bottom - uy * corner))
        slot.addQuadCurve(to: pt(cx + foot - corner, bottom), control: pt(cx + foot, bottom))
        slot.addLine(to: pt(cx - foot + corner, bottom))
        slot.addQuadCurve(to: pt(cx - foot + ux * corner, bottom - uy * corner), control: pt(cx - foot, bottom))
        slot.closeSubpath()

        var bubble = head.union(slot)
        let dot: CGFloat = 39, gap: CGFloat = 114
        for i in -1 ... 1 {
            let x = cx + CGFloat(i) * gap
            bubble.addEllipse(in: CGRect(x: ox + (x - dot) * s, y: oy + (cy - dot) * s, width: 2 * dot * s, height: 2 * dot * s))
        }
        return bubble
    }
}

// MARK: - BlurHash

/// Decodes BlurHash placeholders (https://blurha.sh) so photos show their colors before the
/// thumbnail is decrypted.
public enum BlurHash {
    private static let alphabet = Array("0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~")
    private static let lookup: [Character: Int] = {
        var map: [Character: Int] = [:]
        for (i, c) in alphabet.enumerated() { map[c] = i }
        return map
    }()

    private static func decode83(_ s: Substring) -> Int? {
        var value = 0
        for c in s {
            guard let digit = lookup[c] else { return nil }
            value = value * 83 + digit
        }
        return value
    }

    private static func srgbToLinear(_ v: Int) -> Double {
        let x = Double(v) / 255
        return x <= 0.04045 ? x / 12.92 : pow((x + 0.055) / 1.055, 2.4)
    }

    private static func linearToSrgb(_ v: Double) -> UInt8 {
        let x = max(0, min(1, v))
        let s = x <= 0.003_130_8 ? x * 12.92 : 1.055 * pow(x, 1 / 2.4) - 0.055
        return UInt8(max(0, min(255, Int(s * 255 + 0.5))))
    }

    private static func signPow(_ v: Double, _ e: Double) -> Double {
        copysign(pow(abs(v), e), v)
    }

    /// Decodes into a small image (e.g. 32×32); the view scales it up smoothly.
    public static func image(_ hash: String, width: Int = 32, height: Int = 32, punch: Double = 1) -> NSImage? {
        let chars = Array(hash)
        guard chars.count >= 6, let sizeFlag = decode83(Substring(String(chars[0 ..< 1]))) else { return nil }
        let nx = sizeFlag % 9 + 1
        let ny = sizeFlag / 9 + 1
        guard chars.count == 4 + 2 * nx * ny,
              let quantMax = decode83(Substring(String(chars[1 ..< 2]))),
              let dcValue = decode83(Substring(String(chars[2 ..< 6])))
        else { return nil }
        let maxValue = Double(quantMax + 1) / 166 * punch
        var colors: [(Double, Double, Double)] = [(srgbToLinear(dcValue >> 16), srgbToLinear((dcValue >> 8) & 255), srgbToLinear(dcValue & 255))]
        for i in 1 ..< nx * ny {
            let start = 4 + i * 2
            guard let v = decode83(Substring(String(chars[start ..< start + 2]))) else { return nil }
            let q = (v / 361, (v / 19) % 19, v % 19)
            colors.append((signPow((Double(q.0) - 9) / 9, 2) * maxValue,
                           signPow((Double(q.1) - 9) / 9, 2) * maxValue,
                           signPow((Double(q.2) - 9) / 9, 2) * maxValue))
        }
        var pixels = [UInt8](repeating: 255, count: width * height * 4)
        for y in 0 ..< height {
            for x in 0 ..< width {
                var r = 0.0, g = 0.0, b = 0.0
                for j in 0 ..< ny {
                    for i in 0 ..< nx {
                        let basis = cos(Double.pi * Double(x) * Double(i) / Double(width))
                            * cos(Double.pi * Double(y) * Double(j) / Double(height))
                        let c = colors[i + j * nx]
                        r += c.0 * basis
                        g += c.1 * basis
                        b += c.2 * basis
                    }
                }
                let o = (y * width + x) * 4
                pixels[o] = linearToSrgb(r)
                pixels[o + 1] = linearToSrgb(g)
                pixels[o + 2] = linearToSrgb(b)
            }
        }
        guard let provider = CGDataProvider(data: Data(pixels) as CFData),
              let cg = CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
                               space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                               provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: width, height: height))
    }
}
