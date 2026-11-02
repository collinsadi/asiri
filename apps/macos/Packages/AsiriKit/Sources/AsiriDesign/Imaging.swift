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

/// Asiri's mark: a speech bubble whose tail forms a keyhole, on deep indigo.
public struct AsiriMark: View {
    let size: CGFloat

    public init(size: CGFloat = 64) {
        self.size = size
    }

    public var body: some View {
        ZStack {
            Circle()
                .fill(LinearGradient(colors: [Color(hex: 0x6366F1), Color(hex: 0x3730A3)], startPoint: .topLeading, endPoint: .bottomTrailing))
            KeyholeBubble()
                .fill(Color.white)
                .padding(size * 0.2)
            Keyhole()
                .fill(Color(hex: 0x3F37C9))
                .frame(width: size * 0.17, height: size * 0.3)
                .offset(y: -size * 0.035)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// A rounded bubble whose tail drops from the bottom center.
public struct KeyholeBubble: Shape {
    public init() {}

    public func path(in rect: CGRect) -> Path {
        let w = rect.width
        let h = rect.height
        let bodyH = h * 0.8
        var p = Path(roundedRect: CGRect(x: rect.minX, y: rect.minY, width: w, height: bodyH), cornerRadius: w * 0.3, style: .continuous)
        var tail = Path()
        tail.move(to: CGPoint(x: rect.midX - w * 0.12, y: rect.minY + bodyH - 1))
        tail.addLine(to: CGPoint(x: rect.midX, y: rect.maxY))
        tail.addLine(to: CGPoint(x: rect.midX + w * 0.12, y: rect.minY + bodyH - 1))
        tail.closeSubpath()
        p.addPath(tail)
        return p
    }
}

/// A classic keyhole: a circle over a tapered slot.
public struct Keyhole: Shape {
    public init() {}

    public func path(in rect: CGRect) -> Path {
        let w = rect.width
        let r = w / 2
        var p = Path(ellipseIn: CGRect(x: rect.minX, y: rect.minY, width: w, height: w))
        var slot = Path()
        slot.move(to: CGPoint(x: rect.midX - r * 0.42, y: rect.minY + r))
        slot.addLine(to: CGPoint(x: rect.midX - r * 0.62, y: rect.maxY))
        slot.addLine(to: CGPoint(x: rect.midX + r * 0.62, y: rect.maxY))
        slot.addLine(to: CGPoint(x: rect.midX + r * 0.42, y: rect.minY + r))
        slot.closeSubpath()
        p.addPath(slot)
        return p
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
