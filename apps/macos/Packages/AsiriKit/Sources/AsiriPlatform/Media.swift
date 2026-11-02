import AppKit
import AsiriDomain
import AVFoundation
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// Prepares files for sending (architecture §11.5): photos are re-encoded without metadata (no
/// location, camera or time), get a small thumbnail and a BlurHash; other files go as they are.
public enum MediaPrep {
    public enum PrepError: Error, LocalizedError {
        case unreadable
        case tooLarge

        public var errorDescription: String? {
            switch self {
            case .unreadable: "That file couldn't be read."
            case .tooLarge: "Files can be up to 200 MB."
            }
        }
    }

    public static let maxBytes = 200 * 1024 * 1024

    /// Prepares a file the user picked, dropped or pasted.
    public static func prepare(url: URL) throws -> OutgoingFile {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let values = try? url.resourceValues(forKeys: [.fileSizeKey, .contentTypeKey])
        if let size = values?.fileSize, size > maxBytes { throw PrepError.tooLarge }
        let data = try Data(contentsOf: url, options: .mappedIfSafe)
        let type = values?.contentType ?? UTType(filenameExtension: url.pathExtension) ?? .data
        return try prepare(data: data, type: type, fileName: url.lastPathComponent)
    }

    /// Prepares raw bytes (paste, drag from another app).
    public static func prepare(data: Data, type: UTType, fileName: String) throws -> OutgoingFile {
        guard data.count <= maxBytes else { throw PrepError.tooLarge }
        if type.conforms(to: .image), !type.conforms(to: .gif), let photo = cleanPhoto(data) {
            let base = (fileName as NSString).deletingPathExtension
            return OutgoingFile(data: photo.jpeg, mime: "image/jpeg", fileName: (base.isEmpty ? "Photo" : base) + ".jpg",
                                width: photo.width, height: photo.height, thumbnail: photo.thumbnail, blurhash: photo.blurhash)
        }
        let mime = type.preferredMIMEType ?? "application/octet-stream"
        var file = OutgoingFile(data: data, mime: mime, fileName: fileName)
        if type.conforms(to: .gif), let source = CGImageSourceCreateWithData(data as CFData, nil),
           let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any] {
            file.width = props[kCGImagePropertyPixelWidth] as? Int ?? 0
            file.height = props[kCGImagePropertyPixelHeight] as? Int ?? 0
        }
        return file
    }

    struct CleanPhoto {
        var jpeg: Data
        var width: Int
        var height: Int
        var thumbnail: Data?
        var blurhash: String
    }

    /// Decodes, orients, downsizes to at most 2560 px and re-encodes as JPEG with no metadata.
    static func cleanPhoto(_ data: Data) -> CleanPhoto? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true, // applies EXIF orientation
            kCGImageSourceThumbnailMaxPixelSize: 2560,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary),
              let jpeg = encodeJPEG(image, quality: 0.86)
        else { return nil }
        let thumb = thumbnail(of: image, maxPixel: 320)
        let tiny = thumbnail(cgImage: image, maxPixel: 32)
        return CleanPhoto(jpeg: jpeg, width: image.width, height: image.height,
                          thumbnail: thumb.flatMap { encodeJPEG($0, quality: 0.6) },
                          blurhash: tiny.map { BlurHashEncoder.encode($0) } ?? "")
    }

    private static func thumbnail(of image: CGImage, maxPixel: Int) -> CGImage? {
        thumbnail(cgImage: image, maxPixel: maxPixel)
    }

    private static func thumbnail(cgImage: CGImage, maxPixel: Int) -> CGImage? {
        let scale = min(1, CGFloat(maxPixel) / CGFloat(max(cgImage.width, cgImage.height)))
        let w = max(1, Int(CGFloat(cgImage.width) * scale))
        let h = max(1, Int(CGFloat(cgImage.height) * scale))
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
        else { return nil }
        ctx.interpolationQuality = .high
        ctx.draw(cgImage, in: CGRect(x: 0, y: 0, width: w, height: h))
        return ctx.makeImage()
    }

    /// JPEG with nothing but pixels: no EXIF, GPS, TIFF or maker notes.
    private static func encodeJPEG(_ image: CGImage, quality: CGFloat) -> Data? {
        let out = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(out as CFMutableData, UTType.jpeg.identifier as CFString, 1, nil) else { return nil }
        let props: [CFString: Any] = [kCGImageDestinationLossyCompressionQuality: quality, kCGImageDestinationOptimizeColorForSharing: true]
        CGImageDestinationAddImage(dest, image, props as CFDictionary)
        guard CGImageDestinationFinalize(dest) else { return nil }
        return out as Data
    }
}

/// BlurHash encoder (4×3 components) for photo placeholders.
public enum BlurHashEncoder {
    private static let alphabet = Array("0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~")

    private static func encode83(_ value: Int, length: Int) -> String {
        var out = ""
        for i in 1 ... length {
            let digit = (value / Int(pow(83, Double(length - i)))) % 83
            out.append(alphabet[digit])
        }
        return out
    }

    private static func srgbToLinear(_ v: UInt8) -> Double {
        let x = Double(v) / 255
        return x <= 0.04045 ? x / 12.92 : pow((x + 0.055) / 1.055, 2.4)
    }

    private static func linearToSrgb(_ v: Double) -> Int {
        let x = max(0, min(1, v))
        let s = x <= 0.003_130_8 ? x * 12.92 : 1.055 * pow(x, 1 / 2.4) - 0.055
        return Int(s * 255 + 0.5)
    }

    private static func signPow(_ v: Double, _ e: Double) -> Double { copysign(pow(abs(v), e), v) }

    public static func encode(_ image: CGImage, componentsX: Int = 4, componentsY: Int = 3) -> String {
        let w = image.width
        let h = image.height
        var pixels = [UInt8](repeating: 0, count: w * h * 4)
        let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
            guard let ctx = CGContext(data: buffer.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                      space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
            else { return false }
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        guard drawn else { return "" }
        var factors: [(Double, Double, Double)] = []
        for j in 0 ..< componentsY {
            for i in 0 ..< componentsX {
                let norm: Double = (i == 0 && j == 0) ? 1 : 2
                var r = 0.0, g = 0.0, b = 0.0
                for y in 0 ..< h {
                    for x in 0 ..< w {
                        let basis = norm * cos(Double.pi * Double(i) * Double(x) / Double(w)) * cos(Double.pi * Double(j) * Double(y) / Double(h))
                        let o = (y * w + x) * 4
                        r += basis * srgbToLinear(pixels[o])
                        g += basis * srgbToLinear(pixels[o + 1])
                        b += basis * srgbToLinear(pixels[o + 2])
                    }
                }
                let scale = 1 / Double(w * h)
                factors.append((r * scale, g * scale, b * scale))
            }
        }
        let dc = factors[0]
        let ac = factors.dropFirst()
        var hash = encode83((componentsX - 1) + (componentsY - 1) * 9, length: 1)
        let maxValue: Double
        if let actualMax = ac.map({ max(abs($0.0), abs($0.1), abs($0.2)) }).max(), actualMax > 0 {
            let quantised = max(0, min(82, Int(actualMax * 166 - 0.5)))
            maxValue = Double(quantised + 1) / 166
            hash += encode83(quantised, length: 1)
        } else {
            maxValue = 1
            hash += encode83(0, length: 1)
        }
        hash += encode83((linearToSrgb(dc.0) << 16) + (linearToSrgb(dc.1) << 8) + linearToSrgb(dc.2), length: 4)
        for f in ac {
            func q(_ v: Double) -> Int { max(0, min(18, Int(floor(signPow(v / maxValue, 0.5) * 9 + 9.5)))) }
            hash += encode83(q(f.0) * 19 * 19 + q(f.1) * 19 + q(f.2), length: 2)
        }
        return hash
    }
}

// MARK: - Voice notes

/// Records voice notes (AAC in M4A) and builds their waveform from the live meter.
@MainActor
public final class VoiceRecorder: NSObject, AVAudioRecorderDelegate {
    public private(set) var isRecording = false
    public private(set) var levels: [Float] = []
    private var recorder: AVAudioRecorder?
    private var timer: Timer?
    private var started: Date?
    private var url: URL?

    public var onLevel: ((Float) -> Void)?

    override public init() {
        super.init()
    }

    /// Asks for the microphone (once) and starts recording.
    public func start() async throws {
        let granted = await AVCaptureDevice.requestAccess(for: .audio)
        guard granted else { throw CoreFailure(.notAllowed, "Asiri needs the microphone for voice messages. You can allow it in System Settings → Privacy & Security.") }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("asiri-voice-\(UUID().uuidString).m4a")
        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 32_000,
            AVNumberOfChannelsKey: 1,
            AVEncoderBitRateKey: 48_000,
        ]
        let recorder = try AVAudioRecorder(url: url, settings: settings)
        recorder.isMeteringEnabled = true
        recorder.delegate = self
        guard recorder.record() else { throw CoreFailure(.internal, "The microphone couldn't start.") }
        self.recorder = recorder
        self.url = url
        levels = []
        started = Date()
        isRecording = true
        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, let r = self.recorder else { return }
                r.updateMeters()
                let db = r.averagePower(forChannel: 0)
                let level = max(0, min(1, (db + 50) / 50))
                self.levels.append(level)
                self.onLevel?(level)
            }
        }
    }

    /// Seconds recorded so far.
    public var elapsed: TimeInterval { started.map { Date().timeIntervalSince($0) } ?? 0 }

    /// Stops and returns the voice note, or nil if it was too short.
    public func stop() -> OutgoingFile? {
        guard let recorder, let url else { return nil }
        let duration = recorder.currentTime
        recorder.stop()
        cleanup()
        defer { try? FileManager.default.removeItem(at: url) }
        guard duration >= 0.5, let data = try? Data(contentsOf: url) else { return nil }
        return OutgoingFile(data: data, mime: "audio/mp4", fileName: "Voice message.m4a", durationMs: Int(duration * 1000),
                            isVoiceNote: true, waveform: Self.bucket(levels, into: 48))
    }

    /// Throws the recording away.
    public func cancel() {
        recorder?.stop()
        recorder?.deleteRecording()
        cleanup()
    }

    private func cleanup() {
        timer?.invalidate()
        timer = nil
        recorder = nil
        isRecording = false
        started = nil
    }

    /// Averages levels into `count` bars of 0…255.
    nonisolated public static func bucket(_ levels: [Float], into count: Int) -> [UInt8] {
        guard !levels.isEmpty else { return Array(repeating: 20, count: count) }
        return (0 ..< count).map { i in
            let lo = i * levels.count / count
            let hi = max(lo + 1, (i + 1) * levels.count / count)
            let slice = levels[lo ..< min(hi, levels.count)]
            let avg = slice.reduce(0, +) / Float(max(slice.count, 1))
            return UInt8(max(12, min(255, avg * 255)))
        }
    }
}

/// Plays a voice note from memory (the plaintext never touches the disk).
@MainActor
public final class VoicePlayer: NSObject, AVAudioPlayerDelegate {
    public static let shared = VoicePlayer()
    public private(set) var playing: MessageID?
    private var player: AVAudioPlayer?
    public var onChange: (() -> Void)?

    public var progress: Double {
        guard let p = player, p.duration > 0 else { return 0 }
        return p.currentTime / p.duration
    }

    public func toggle(_ id: MessageID, data: Data) {
        if playing == id {
            stop()
            return
        }
        stop()
        guard let p = try? AVAudioPlayer(data: data) else { return }
        p.delegate = self
        p.play()
        player = p
        playing = id
        onChange?()
    }

    public func stop() {
        player?.stop()
        player = nil
        playing = nil
        onChange?()
    }

    nonisolated public func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        Task { @MainActor in self.stop() }
    }
}
