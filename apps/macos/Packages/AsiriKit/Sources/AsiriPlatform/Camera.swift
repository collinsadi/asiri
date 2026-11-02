import AppKit
import AVFoundation
import Vision

/// A live camera preview that reads Asiri pairing codes (architecture §11.2). The preview is
/// mirrored, so holding the two Macs face to face feels natural. Frames are analysed on-device
/// with Vision and never stored.
public final class QRScannerView: NSView, AVCaptureVideoDataOutputSampleBufferDelegate {
    public enum ScanState: Equatable {
        case starting
        case scanning
        case denied
        case noCamera
    }

    public var onCode: ((String) -> Void)?
    public var onState: ((ScanState) -> Void)?

    private let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "asiri.qr-scanner")
    private var preview: AVCaptureVideoPreviewLayer?
    private var lastCode: String?
    private var running = false

    override public init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
        layer?.cornerRadius = 18
        layer?.masksToBounds = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override public func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil { start() } else { stop() }
    }

    override public func layout() {
        super.layout()
        preview?.frame = bounds
    }

    public func start() {
        guard !running else { return }
        running = true
        onState?(.starting)
        AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
            DispatchQueue.main.async {
                guard let self else { return }
                guard granted else {
                    self.onState?(.denied)
                    return
                }
                self.configure()
            }
        }
    }

    public func stop() {
        running = false
        let session = self.session
        queue.async { if session.isRunning { session.stopRunning() } }
    }

    private func configure() {
        let discovery = AVCaptureDevice.DiscoverySession(
            deviceTypes: [.builtInWideAngleCamera, .external, .continuityCamera],
            mediaType: .video,
            position: .unspecified
        )
        guard let device = discovery.devices.first ?? AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device)
        else {
            onState?(.noCamera)
            return
        }
        session.beginConfiguration()
        session.sessionPreset = .high
        if session.canAddInput(input) { session.addInput(input) }
        let output = AVCaptureVideoDataOutput()
        output.alwaysDiscardsLateVideoFrames = true
        output.setSampleBufferDelegate(self, queue: queue)
        if session.canAddOutput(output) { session.addOutput(output) }
        session.commitConfiguration()

        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        layer.frame = bounds
        if let connection = layer.connection, connection.isVideoMirroringSupported {
            connection.automaticallyAdjustsVideoMirroring = false
            connection.isVideoMirrored = true
        }
        self.layer?.addSublayer(layer)
        preview = layer
        let session = self.session
        queue.async { session.startRunning() }
        onState?(.scanning)
    }

    public func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        guard let pixels = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        let request = VNDetectBarcodesRequest()
        request.symbologies = [.qr]
        let handler = VNImageRequestHandler(cvPixelBuffer: pixels, options: [:])
        try? handler.perform([request])
        guard let code = request.results?.compactMap({ $0.payloadStringValue }).first(where: { $0.hasPrefix("ASIRI1:") }) else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.lastCode != code else { return }
            self.lastCode = code
            self.onCode?(code)
        }
    }
}
