import AVFoundation
import CoreMedia
import CoreVideo
import Foundation
import UIKit

/// Tracks whether the app is in the foreground: iOS only delivers camera
/// frames while the app is active, so `Start` must be refused otherwise.
final class AppActivity {
    static let shared = AppActivity()
    private(set) var isForeground = true

    private init() {
        let center = NotificationCenter.default
        _ = center.addObserver(
            forName: UIApplication.didBecomeActiveNotification,
            object: nil,
            queue: .main
        ) { _ in AppActivity.shared.isForeground = true }
        _ = center.addObserver(
            forName: UIApplication.willResignActiveNotification,
            object: nil,
            queue: .main
        ) { _ in AppActivity.shared.isForeground = false }
    }
}

/// Per-connection protocol state machine: turns the PC's commands into camera
/// and encoder work, and streams `VideoConfig` / `VideoFrame` / `Status` back.
///
/// Everything runs on the server's serial queue (also used as the capture
/// delegate queue), so no additional locking is needed.
final class LinkSession {
    private let queue: DispatchQueue
    private let send: (WclFrame) -> Void
    private let pendingBytes: () -> Int
    private let log: (String) -> Void
    private let camera: CameraController
    private let cameras: [WclCameraDescriptor]

    private var selectedCameraId: UInt8 = 0
    private var format = WclSetFormat(
        width: 1920,
        height: 1080,
        fps: 30,
        codec: WclCodec.h264.rawValue,
        bitrateKbps: 20_000
    )
    private var options = WclSetOptions(mirror: false, rotation: 0, dimPreview: false)
    private var image = WclSetImage()

    private var encoder: H264Encoder?
    private var encodedWidth: UInt16 = 0
    private var encodedHeight: UInt16 = 0
    private var streaming = false
    private var configurationSent = false
    private var statusTimer: DispatchSourceTimer?
    private var statusWindowStart = Date()
    private var bytesInWindow = 0
    private var framesInWindow = 0
    private var droppedFrames = 0

    /// Mirror of the stream state for the UI.
    var onStreamingChanged: ((Bool) -> Void)?
    /// Last `Status` summary for the UI.
    var onStatusText: ((String) -> Void)?

    /// Drop non-key frames once this much data is still queued on the socket.
    private static let backpressureLimit = 4 * 1024 * 1024

    init(
        queue: DispatchQueue,
        cameras: [WclCameraDescriptor],
        send: @escaping (WclFrame) -> Void,
        pendingBytes: @escaping () -> Int,
        log: @escaping (String) -> Void
    ) {
        self.queue = queue
        self.cameras = cameras
        self.send = send
        self.pendingBytes = pendingBytes
        self.log = log
        self.camera = CameraController(queue: queue)
        if let first = cameras.first {
            self.selectedCameraId = first.cameraId
        }

        camera.log = { [weak self] message in self?.log(message) }
        camera.onSampleBuffer = { [weak self] sampleBuffer in
            self?.handleSampleBuffer(sampleBuffer)
        }

        // iOS suspends the camera as soon as the app leaves the foreground:
        // tell the PC instead of letting it wait on a silent stream.
        _ = NotificationCenter.default.addObserver(
            forName: UIApplication.willResignActiveNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            guard let self else { return }
            self.queue.async {
                // Tell the PC even when idle: the listener is about to be
                // suspended, so the next Connect will be refused anyway.
                self.sendError(.notInForeground, message: "l'app iPhone est passée en arrière-plan")
                self.stop(reason: "arrière-plan")
            }
        }
    }

    func describeCaps() -> WclCaps {
        let cameras = self.cameras.map { descriptor in
            WclCamera(
                cameraId: descriptor.cameraId,
                position: descriptor.position,
                lens: descriptor.lens,
                label: descriptor.label,
                formats: descriptor.formats
            )
        }
        return WclCaps(cameras: cameras, codecs: WclCapsCodec.h264)
    }

    /// Ranges of the selected camera. Re-sent after every `SetCamera`, because
    /// they describe that camera and not the device as a whole.
    func describeImageCaps() -> WclImageCaps {
        cameras.first { $0.cameraId == selectedCameraId }?.imageCaps ?? WclImageCaps()
    }

    // MARK: - Incoming commands

    func handle(frame: WclFrame) {
        switch frame.type {
        case .hello:
            // Answer every Hello: our own Hello was already sent on connect,
            // but the PC may re-announce.
            if let hello = WclHello.decode(frame.payload) {
                log("Hello reçu — role=\(hello.role) app=\(hello.appVersion) name=« \(hello.name) » model=\(hello.model)")
            }
            sendHello()

        case .setCamera:
            guard let command = WclSetCamera.decode(frame.payload) else {
                log("SetCamera illisible")
                return
            }
            guard cameras.contains(where: { $0.cameraId == command.cameraId }) else {
                sendError(.invalidFormat, message: "caméra \(command.cameraId) inconnue")
                return
            }
            selectedCameraId = command.cameraId
            log("SetCamera(\(command.cameraId))")
            send(WclFrame(type: .imageCaps, payload: describeImageCaps().encoded()))

        case .setFormat:
            guard let requested = WclSetFormat.decode(frame.payload) else {
                log("SetFormat illisible")
                return
            }
            guard requested.codec == WclCodec.h264.rawValue else {
                sendError(.invalidFormat, message: "codec \(requested.codec) non supporté en v1")
                return
            }
            format = requested
            log("SetFormat \(requested.width)x\(requested.height) @ \(requested.fps) fps, \(requested.bitrateKbps) kbps")

        case .setOptions:
            guard let requested = WclSetOptions.decode(frame.payload) else { return }
            options = requested
            camera.apply(connection: requested.mirror, rotation: requested.rotation)
            log("SetOptions(mirror: \(requested.mirror), rotation: \(requested.rotation), dim: \(requested.dimPreview))")

        case .setImage:
            image = image.updated(with: frame.payload)
            do {
                try camera.apply(image: image)
                log(
                    "SetImage: zoom \(image.zoomX100)%, expo mode \(image.exposureMode) "
                    + "(biais \(image.exposureBiasEvX100), ISO \(image.iso), \(image.exposureDurationUs) µs), "
                    + "focus mode \(image.focusMode) (\(image.focusXX10000), \(image.focusYX10000)), "
                    + "WB mode \(image.whiteBalanceMode) \(image.whiteBalanceTempK) K "
                    + "teinte \(image.whiteBalanceTintX100), torche \(image.torch) \(image.torchLevel)%"
                )
            } catch {
                // A camera that cannot do the requested mode is the PC's problem
                // to report: it knows what it asked for.
                sendError(.imageUnsupported, message: "\(error)")
            }

        case .start:
            start()

        case .stop:
            stop(reason: "Stop reçu")

        case .requestKeyframe:
            encoder?.requestKeyframe()
            log("RequestKeyframe")

        case .heartbeat:
            var reader = WclReader(frame.payload)
            guard let stamp = reader.u64() else { return }
            var writer = WclWriter()
            writer.u64(stamp)
            send(WclFrame(type: .heartbeat, payload: writer.data))

        default:
            log("Message \(frame.type) ignoré")
        }
    }

    func sendHello() {
        let hello = WclHello(
            role: 1,
            appVersion: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0",
            name: UIDevice.current.name,
            model: LinkServer.deviceModel()
        )
        send(WclFrame(type: .hello, payload: hello.encoded()))
        send(WclFrame(type: .caps, payload: describeCaps().encoded()))
        send(WclFrame(type: .imageCaps, payload: describeImageCaps().encoded()))
    }

    // MARK: - Streaming

    private func start() {
        guard !streaming else { return }
        guard AppActivity.shared.isForeground else {
            sendError(.notInForeground, message: "l'app iPhone doit être au premier plan")
            return
        }
        let authorization = AVCaptureDevice.authorizationStatus(for: .video)
        guard authorization != .denied, authorization != .restricted else {
            sendError(.cameraPermissionDenied, message: "accès caméra refusé dans Réglages")
            return
        }
        guard let descriptor = cameras.first(where: { $0.cameraId == selectedCameraId }) else {
            sendError(.invalidFormat, message: "aucune caméra sélectionnée")
            return
        }

        let width = Int(format.width)
        let height = Int(format.height)
        let fps = Int(format.fps)
        guard width > 0, height > 0, fps > 0 else {
            sendError(.invalidFormat, message: "format invalide")
            return
        }

        do {
            try camera.configure(descriptor: descriptor, width: width, height: height, fps: fps, mirror: options.mirror)
            do {
                try camera.apply(image: image)
            } catch {
                // An unsupported image mode must not keep the stream from
                // starting: tell the PC, keep the rest of the settings.
                sendError(.imageUnsupported, message: "\(error)")
            }
        } catch {
            sendError(.invalidFormat, message: "\(error)")
            return
        }

        // The encoder is created from the first delivered buffer: the capture
        // connection may rotate the image (portrait delivery), so the encoded
        // size is the buffer size, not necessarily the negotiated format.
        encoder = nil
        encodedWidth = 0
        encodedHeight = 0

        do {
            try camera.start()
        } catch {
            sendError(.cameraUnavailable, message: "\(error)")
            return
        }

        streaming = true
        configurationSent = false
        bytesInWindow = 0
        framesInWindow = 0
        droppedFrames = 0
        statusWindowStart = Date()
        startStatusTimer()
        onStreamingChanged?(true)
        log("Flux démarré: \(width)x\(height) @ \(fps) fps, \(format.bitrateKbps) kbps")
    }

    private func stop(reason: String) {
        guard streaming else { return }
        streaming = false
        camera.stop()
        encoder?.finish()
        encoder = nil
        stopStatusTimer()
        onStreamingChanged?(false)
        log("Flux arrêté (\(reason))")
    }

    func close() {
        stop(reason: "connexion fermée")
    }

    /// The camera delivers NV12 buffers; encoding stays on this queue.
    private func handleSampleBuffer(_ sampleBuffer: CMSampleBuffer) {
        guard streaming else { return }
        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }

        let bufferWidth = UInt16(clamping: CVPixelBufferGetWidth(pixelBuffer))
        let bufferHeight = UInt16(clamping: CVPixelBufferGetHeight(pixelBuffer))
        guard bufferWidth > 0, bufferHeight > 0 else { return }

        if encoder == nil || bufferWidth != encodedWidth || bufferHeight != encodedHeight {
            encoder?.finish()
            encoder = nil
            do {
                let created = try H264Encoder(
                    width: Int(bufferWidth),
                    height: Int(bufferHeight),
                    fps: Int(format.fps),
                    bitrateKbps: format.bitrateKbps
                )
                created.log = { [weak self] message in self?.log(message) }
                created.onAccessUnit = { [weak self] data, ptsUs, keyframe in
                    guard let self else { return }
                    self.queue.async { self.publish(data: data, ptsUs: ptsUs, keyframe: keyframe) }
                }
                encoder = created
                encodedWidth = bufferWidth
                encodedHeight = bufferHeight
                configurationSent = false
                log("Encodeur prêt: \(bufferWidth)x\(bufferHeight)")
            } catch {
                sendError(.encoderFailure, message: "\(error)")
                stop(reason: "échec de l'encodeur")
                return
            }
        }

        encoder?.encode(pixelBuffer: pixelBuffer, presentationTime: CMSampleBufferGetPresentationTimeStamp(sampleBuffer))
    }

    private func publish(data: Data, ptsUs: UInt64, keyframe: Bool) {
        guard streaming else { return }

        // First picture: announce the stream (parameter sets are known now).
        if !configurationSent, let encoder {
            let config = WclVideoConfig(
                codec: WclCodec.h264.rawValue,
                width: encodedWidth,
                height: encodedHeight,
                fps: format.fps,
                parameterSets: encoder.parameterSets
            )
            send(WclFrame(type: .videoConfig, payload: config.encoded()))
            configurationSent = true
            log("VideoConfig envoyé (\(encodedWidth)x\(encodedHeight), \(encoder.parameterSets.count) octets de paramètres)")
        }

        // Backpressure: keep the link responsive by dropping the non-essential
        // frames when the socket queue backs up.
        if !keyframe && pendingBytes() > Self.backpressureLimit {
            droppedFrames += 1
            return
        }

        let frame = WclVideoFrame(
            ptsUs: ptsUs,
            flags: keyframe ? WclFrameFlags.keyframe : 0,
            data: data
        )
        let payload = frame.encoded()
        bytesInWindow += payload.count
        framesInWindow += 1
        send(WclFrame(type: .videoFrame, payload: payload))
    }

    // MARK: - Status

    private func startStatusTimer() {
        stopStatusTimer()
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 2, repeating: 2)
        timer.setEventHandler { [weak self] in self?.publishStatus() }
        timer.resume()
        statusTimer = timer
    }

    private func stopStatusTimer() {
        statusTimer?.cancel()
        statusTimer = nil
    }

    private func publishStatus() {
        guard streaming else { return }
        let elapsed = max(0.5, Date().timeIntervalSince(statusWindowStart))
        let bitrate = UInt32(Double(bytesInWindow) * 8.0 / 1000.0 / elapsed)
        let fps = UInt16(min(Double(framesInWindow) / elapsed * 100.0, Double(UInt16.max)))

        UIDevice.current.isBatteryMonitoringEnabled = true
        let level = UIDevice.current.batteryLevel
        let battery: UInt8 = level >= 0 ? UInt8(max(0, min(100, level * 100))) : 255
        let charging: Bool
        switch UIDevice.current.batteryState {
        case .charging, .full: charging = true
        default: charging = false
        }

        let status = WclStatus(
            thermalState: Self.thermalStateCode(),
            batteryPercent: battery,
            charging: charging,
            actualBitrateKbps: bitrate,
            actualFpsX100: fps,
            cameraId: selectedCameraId
        )
        send(WclFrame(type: .status, payload: status.encoded()))
        onStatusText?(
            "\(bitrate) kbps · \(Double(fps) / 100.0) fps · thermique \(status.thermalState) · batterie \(battery)"
        )

        if droppedFrames > 0 {
            log("Status: \(bitrate) kbps, \(Double(fps) / 100.0) fps, \(droppedFrames) frame(s) écartée(s)")
            droppedFrames = 0
        }

        bytesInWindow = 0
        framesInWindow = 0
        statusWindowStart = Date()

        // Thermal policy (v1): stop when the iPhone reports a critical state.
        if ProcessInfo.processInfo.thermalState == .critical {
            sendError(.thermalShutdown, message: "iPhone en surchauffe — flux arrêté")
            stop(reason: "surchauffe")
        }
    }

    private static func thermalStateCode() -> UInt8 {
        switch ProcessInfo.processInfo.thermalState {
        case .nominal: return 0
        case .fair: return 1
        case .serious: return 2
        case .critical: return 3
        @unknown default: return 0
        }
    }

    // MARK: - Errors

    private func sendError(_ code: WclErrorCode, message: String) {
        log("Erreur envoyée: \(code) — \(message)")
        let error = WclProtocolError(code: code, message: message)
        send(WclFrame(type: .error, payload: error.encoded()))
    }
}
