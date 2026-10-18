import AVFoundation
import CoreMedia
import Foundation

/// One camera exposed to the PC; mirrors the `Caps` message. `imageCaps` is the
/// range block announced to the PC (message `ImageCaps`) — see `discover()`.
struct WclCameraDescriptor {
    var cameraId: UInt8
    var position: UInt8
    var lens: UInt8
    var label: String
    var formats: [WclCameraFormat]
    var imageCaps: WclImageCaps
    var device: AVCaptureDevice
}

enum CameraError: Error, CustomStringConvertible {
    case permissionDenied
    case noSuchCamera
    case noSuchFormat(String)
    case cannotConfigure(String)
    case unsupportedControl(String)

    var description: String {
        switch self {
        case .permissionDenied: return "accès caméra refusé"
        case .noSuchCamera: return "caméra inconnue"
        case .noSuchFormat(let detail): return "format indisponible (\(detail))"
        case .cannotConfigure(let detail): return "configuration refusée (\(detail))"
        case .unsupportedControl(let detail): return "réglage non supporté (\(detail))"
        }
    }
}

/// Owns the capture session: discovery, format selection and the NV12 frame
/// callback. Everything runs on the serial queue handed in at init, so the
/// session state machine needs no extra locking.
final class CameraController: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    private let session = AVCaptureSession()
    private let output = AVCaptureVideoDataOutput()
    private let queue: DispatchQueue
    private var currentInput: AVCaptureDeviceInput?
    private(set) var isRunning = false
    private(set) var descriptor: WclCameraDescriptor?

    var onSampleBuffer: ((CMSampleBuffer) -> Void)?
    var log: ((String) -> Void)?

    init(queue: DispatchQueue) {
        self.queue = queue
        super.init()
    }

    // MARK: - Discovery

    /// All individual lenses (no virtual multi-camera devices), each with its
    /// deduplicated (width, height) list and the best frame rate available.
    static func discover() -> [WclCameraDescriptor] {
        let discovery = AVCaptureDevice.DiscoverySession(
            deviceTypes: [.builtInWideAngleCamera, .builtInUltraWideCamera, .builtInTelephotoCamera],
            mediaType: .video,
            position: .unspecified
        )

        var descriptors: [WclCameraDescriptor] = []
        for (index, device) in discovery.devices.enumerated() {
            let lens: WclLens
            switch device.deviceType {
            case .builtInUltraWideCamera: lens = .ultraWide
            case .builtInTelephotoCamera: lens = .telephoto
            default: lens = device.position == .front ? .front : .wide
            }

            var bestFps: [String: Int] = [:]
            var order: [String] = []
            for format in device.formats {
                let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
                guard dimensions.width > 0, dimensions.height > 0 else { continue }
                var maxFps = 0.0
                for range in format.videoSupportedFrameRateRanges {
                    maxFps = max(maxFps, range.maxFrameRate)
                }
                guard maxFps >= 1 else { continue }
                // Slow-motion presets (120/240 fps) are not useful for a webcam.
                let fps = Int(min(maxFps.rounded(), 60))
                let key = "\(dimensions.width)x\(dimensions.height)"
                if let existing = bestFps[key] {
                    bestFps[key] = max(existing, fps)
                } else {
                    bestFps[key] = fps
                    order.append(key)
                }
            }

            var formats: [WclCameraFormat] = []
            for key in order {
                let parts = key.split(separator: "x")
                guard parts.count == 2,
                      let width = UInt16(parts[0]),
                      let height = UInt16(parts[1]),
                      let fps = bestFps[key] else { continue }
                formats.append(WclCameraFormat(width: width, height: height, maxFps: UInt16(fps)))
            }

            descriptors.append(
                WclCameraDescriptor(
                    cameraId: UInt8(index),
                    position: device.position == .front ? WclPosition.front.rawValue : WclPosition.back.rawValue,
                    lens: lens.rawValue,
                    label: device.localizedName,
                    formats: formats,
                    imageCaps: Self.imageCaps(for: device),
                    device: device
                )
            )
        }
        return descriptors
    }

    // MARK: - Configuration

    /// Widest ranges across the device's formats. Announcing generously is safe:
    /// `apply(image:)` clamps again against the format that ends up active, and
    /// the PC shows the range it was given.
    private static func imageCaps(for device: AVCaptureDevice) -> WclImageCaps {
        var caps = WclImageCaps()

        var isoMin = Double.greatestFiniteMagnitude
        var isoMax = 0.0
        var durationMinUs = Double.greatestFiniteMagnitude
        var durationMaxUs = 0.0
        var zoomMax = 1.0
        for format in device.formats {
            if format.minISO > 0, format.maxISO > format.minISO {
                isoMin = min(isoMin, Double(format.minISO))
                isoMax = max(isoMax, Double(format.maxISO))
            }
            let minUs = CMTimeGetSeconds(format.minExposureDuration) * 1_000_000
            let maxUs = CMTimeGetSeconds(format.maxExposureDuration) * 1_000_000
            if minUs.isFinite, maxUs.isFinite, minUs > 0, maxUs > minUs {
                durationMinUs = min(durationMinUs, minUs)
                durationMaxUs = max(durationMaxUs, maxUs)
            }
            let formatZoom = Double(format.videoMaxZoomFactor)
            if formatZoom.isFinite {
                zoomMax = max(zoomMax, formatZoom)
            }
        }

        if device.hasTorch, device.isTorchModeSupported(.on) {
            caps.flags |= WclImageCapsFlags.torch
            caps.flags |= WclImageCapsFlags.torchLevel
        }
        if zoomMax > 1.0 {
            caps.flags |= WclImageCapsFlags.zoom
            caps.zoomMaxX100 = UInt16(min(zoomMax * 100.0, Double(UInt16.max)).rounded())
        }
        if isoMax > isoMin {
            caps.flags |= WclImageCapsFlags.iso
            caps.isoMin = UInt16(min(isoMin, Double(UInt16.max)).rounded())
            caps.isoMax = UInt16(min(isoMax, Double(UInt16.max)).rounded())
        }
        if durationMaxUs > durationMinUs {
            caps.flags |= WclImageCapsFlags.exposureDuration
            caps.exposureMinUs = UInt32(min(durationMinUs, Double(UInt32.max)).rounded())
            caps.exposureMaxUs = UInt32(min(durationMaxUs, Double(UInt32.max)).rounded())
        }
        if device.isWhiteBalanceModeSupported(.locked),
           device.isWhiteBalanceModeSupported(.continuousAutoWhiteBalance) {
            caps.flags |= WclImageCapsFlags.whiteBalance
            caps.whiteBalanceMinK = 2000
            caps.whiteBalanceMaxK = 9000
            caps.whiteBalanceTintMinX100 = -150
            caps.whiteBalanceTintMaxX100 = 150
        }
        if device.isFocusPointOfInterestSupported, device.isFocusModeSupported(.autoFocus) {
            caps.flags |= WclImageCapsFlags.focusPoint
        }
        if device.isFocusModeSupported(.autoFocus), device.isFocusModeSupported(.locked) {
            caps.flags |= WclImageCapsFlags.focusDistance
        }
        for (mode, flag) in [
            (AVCaptureVideoStabilizationMode.off, WclStabilizationModes.off),
            (.standard, WclStabilizationModes.standard),
            (.cinematic, WclStabilizationModes.cinematic),
        ] where device.activeFormat.isVideoStabilizationModeSupported(mode) {
            caps.stabilizationModes |= flag
        }
        if caps.stabilizationModes != 0 {
            caps.flags |= WclImageCapsFlags.stabilization
        }
        return caps
    }

    /// Applies the requested (width, height, fps) to the device.
    func configure(descriptor: WclCameraDescriptor, width: Int, height: Int, fps: Int, mirror: Bool) throws {
        let device = descriptor.device
        guard let format = Self.format(of: device, width: width, height: height, fps: fps) else {
            throw CameraError.noSuchFormat("\(width)x\(height) @ \(fps) fps")
        }

        do {
            try device.lockForConfiguration()
        } catch {
            throw CameraError.cannotConfigure(error.localizedDescription)
        }
        device.activeFormat = format
        let duration = CMTime(value: 1, timescale: CMTimeScale(max(1, fps)))
        device.activeVideoMinFrameDuration = duration
        device.activeVideoMaxFrameDuration = duration
        device.unlockForConfiguration()

        self.descriptor = descriptor
        rebuildInputIfNeeded(device: device, mirror: mirror)
        log?("Format appliqué: \(width)x\(height) @ \(fps) fps (\(device.localizedName))")
    }

    /// Exact (width, height) match with the best frame rate ≥ `fps`.
    private static func format(of device: AVCaptureDevice, width: Int, height: Int, fps: Int) -> AVCaptureDevice.Format? {
        var best: AVCaptureDevice.Format?
        var bestFps = 0.0
        for format in device.formats {
            let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
            guard Int(dimensions.width) == width, Int(dimensions.height) == height else { continue }
            var maxFps = 0.0
            for range in format.videoSupportedFrameRateRanges {
                maxFps = max(maxFps, range.maxFrameRate)
            }
            guard maxFps >= Double(fps) else { continue }
            if best == nil || maxFps < bestFps {
                best = format
                bestFps = maxFps
            }
        }
        return best ?? device.formats.first { format in
            let dimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
            return Int(dimensions.width) == width && Int(dimensions.height) == height
        }
    }

    private func rebuildInputIfNeeded(device: AVCaptureDevice, mirror: Bool) {
        if let input = currentInput, input.device.uniqueID == device.uniqueID {
            apply(connection: mirror, rotation: nil)
            return
        }

        session.beginConfiguration()
        session.sessionPreset = .inputPriority
        if let input = currentInput {
            session.removeInput(input)
            currentInput = nil
        }
        do {
            let input = try AVCaptureDeviceInput(device: device)
            guard session.canAddInput(input) else {
                session.commitConfiguration()
                log?("Impossible d'ajouter l'entrée caméra")
                return
            }
            session.addInput(input)
            currentInput = input
        } catch {
            session.commitConfiguration()
            log?("AVCaptureDeviceInput: \(error.localizedDescription)")
            return
        }

        if !session.outputs.contains(output) {
            output.alwaysDiscardsLateVideoFrames = true
            output.videoSettings = [
                kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            ]
            guard session.canAddOutput(output) else {
                session.commitConfiguration()
                log?("Impossible d'ajouter la sortie vidéo")
                return
            }
            session.addOutput(output)
        }
        session.commitConfiguration()
        apply(connection: mirror, rotation: nil)
    }

    /// Mirroring and orientation for the video output.
    func apply(connection mirror: Bool, rotation: UInt16?) {
        guard let connection = output.connection(with: .video) else { return }
        if connection.isVideoMirroringSupported {
            connection.automaticallyAdjustsVideoMirroring = false
            connection.isVideoMirrored = mirror
        }
        if let rotation, connection.isVideoOrientationSupported {
            switch rotation {
            case 90: connection.videoOrientation = .landscapeLeft
            case 180: connection.videoOrientation = .portraitUpsideDown
            case 270: connection.videoOrientation = .landscapeRight
            default: connection.videoOrientation = .portrait
            }
        }
    }

    // MARK: - Start / stop

    func start() throws {
        if currentInput == nil {
            throw CameraError.noSuchFormat("aucune caméra configurée")
        }
        output.setSampleBufferDelegate(self, queue: queue)
        if !session.isRunning {
            session.startRunning()
        }
        isRunning = session.isRunning
        log?("Capture démarrée (\(isRunning ? "ok" : "échec"))")
    }

    func stop() {
        // Leaving the torch on after a stop would be a surprise: the phone stays
        // lit and heats up, so turn it off before releasing the session.
        if let device = descriptor?.device, device.hasTorch, device.torchMode != .off {
            if (try? device.lockForConfiguration()) != nil {
                device.torchMode = .off
                device.unlockForConfiguration()
            }
        }
        if session.isRunning {
            session.stopRunning()
        }
        isRunning = false
        log?("Capture arrêtée")
    }

    // MARK: - Image controls

    /// Pushes every image control to the device, in one locked section.
    ///
    /// Throws `unsupportedControl` instead of silently ignoring a mode the
    /// camera cannot do: the PC reports it to the user. Values are clamped
    /// against the active format here, because the announced ranges are the
    /// widest ones and ISO/duration narrow down when the format or the zoom
    /// changes.
    func apply(image: WclSetImage) throws {
        guard let device = descriptor?.device else { return }

        do {
            try device.lockForConfiguration()
        } catch {
            throw CameraError.cannotConfigure(error.localizedDescription)
        }
        defer { device.unlockForConfiguration() }

        // Torch: graded when the device takes a level, plain otherwise.
        if device.hasTorch, device.isTorchAvailable, device.isTorchModeSupported(.on) {
            if image.torch {
                let level = min(max(Double(image.torchLevel) / 100.0, 0.05), 1.0)
                try? device.setTorchModeOn(level: Float(level))
            } else {
                device.torchMode = .off
            }
        }

        // Zoom, bounded by both ceilings (format and device).
        let zoomMax = min(
            Double(device.activeFormat.videoMaxZoomFactor),
            Double(device.maxAvailableVideoZoomFactor)
        )
        if zoomMax > 1.0 {
            device.videoZoomFactor = CGFloat(max(1.0, min(Double(image.zoomX100) / 100.0, zoomMax)))
        }

        // Exposure. ISO and duration are set together: `setExposureModeCustom`
        // takes both, and neither has a meaning on its own.
        switch image.exposureMode {
        case WclExposureMode.manual:
            guard device.isExposureModeSupported(.custom) else {
                throw CameraError.unsupportedControl("exposition manuelle")
            }
            let isoMin = Double(device.activeFormat.minISO)
            let isoMax = Double(device.activeFormat.maxISO)
            let durationMin = CMTimeGetSeconds(device.activeFormat.minExposureDuration)
            let durationMax = CMTimeGetSeconds(device.activeFormat.maxExposureDuration)
            let requestedIso = image.iso > 0 ? Double(image.iso) : isoMin
            let requestedSeconds = image.exposureDurationUs > 0
                ? Double(image.exposureDurationUs) / 1_000_000.0
                : durationMin
            let iso = Float(max(isoMin, min(requestedIso, isoMax)))
            let seconds = max(durationMin, min(requestedSeconds, durationMax))
            device.setExposureModeCustom(
                duration: CMTime(seconds: seconds, preferredTimescale: 1_000_000_000),
                iso: iso,
                completionHandler: nil
            )
        case WclExposureMode.bias:
            guard device.isExposureModeSupported(.continuousAutoExposure) else {
                throw CameraError.unsupportedControl("exposition automatique")
            }
            device.exposureMode = .continuousAutoExposure
            let bias = max(
                Double(device.minExposureTargetBias),
                min(Double(image.exposureBiasEvX100) / 100.0, Double(device.maxExposureTargetBias))
            )
            device.setExposureTargetBias(Float(bias), completionHandler: nil)
        default:
            guard device.isExposureModeSupported(.continuousAutoExposure) else {
                throw CameraError.unsupportedControl("exposition automatique")
            }
            device.exposureMode = .continuousAutoExposure
        }

        // White balance: automatic, locked at the current gains, or manual.
        switch image.whiteBalanceMode {
        case WclWhiteBalanceMode.locked, WclWhiteBalanceMode.manual:
            guard device.isWhiteBalanceModeSupported(.locked) else {
                throw CameraError.unsupportedControl("balance des blancs verrouillée")
            }
            var gains = device.deviceWhiteBalanceGains
            if image.whiteBalanceMode == WclWhiteBalanceMode.manual, image.whiteBalanceTempK > 0 {
                let requested = AVCaptureDevice.WhiteBalanceTemperatureAndTintValues(
                    temperature: Float(image.whiteBalanceTempK),
                    tint: Float(image.whiteBalanceTintX100) / 100.0
                )
                gains = device.deviceWhiteBalanceGains(for: requested)
            }
            device.setWhiteBalanceModeLocked(
                with: Self.clamped(gains, maxGain: device.maxWhiteBalanceGain),
                completionHandler: nil
            )
        default:
            guard device.isWhiteBalanceModeSupported(.continuousAutoWhiteBalance) else {
                throw CameraError.unsupportedControl("balance des blancs automatique")
            }
            device.whiteBalanceMode = .continuousAutoWhiteBalance
        }

        // Focus: continuous, locked at a distance, or on a point of interest.
        let point = CGPoint(
            x: min(max(Double(image.focusXX10000) / 10000.0, 0), 1),
            y: min(max(Double(image.focusYX10000) / 10000.0, 0), 1)
        )
        switch image.focusMode {
        case WclFocusMode.locked:
            guard device.isFocusModeSupported(.autoFocus) else {
                throw CameraError.unsupportedControl("mise au point manuelle")
            }
            let distance = Float(min(max(Double(image.focusDistanceX100) / 100.0, 0), 1))
            device.setFocusModeLocked(lensPosition: distance, completionHandler: nil)
        case WclFocusMode.point:
            guard device.isFocusModeSupported(.autoFocus), device.isFocusPointOfInterestSupported else {
                throw CameraError.unsupportedControl("point de mise au point")
            }
            device.focusPointOfInterest = point
            device.focusMode = .autoFocus
        default:
            guard device.isFocusModeSupported(.continuousAutoFocus) else {
                throw CameraError.unsupportedControl("autofocus continu")
            }
            if device.isFocusPointOfInterestSupported {
                device.focusPointOfInterest = point
            }
            device.focusMode = .continuousAutoFocus
        }

        // Stabilisation, when the format offers the requested mode. It is a
        // property of the video connection, not of the device.
        let stabilization: AVCaptureVideoStabilizationMode
        switch image.stabilization {
        case WclStabilization.standard: stabilization = .standard
        case WclStabilization.cinematic: stabilization = .cinematic
        default: stabilization = .off
        }
        if device.activeFormat.isVideoStabilizationModeSupported(stabilization),
           let connection = output.connection(with: .video),
           connection.isVideoStabilizationSupported {
            connection.preferredVideoStabilizationMode = stabilization
        }
    }

    /// Gains are capped by the device and `greenGain` is always 1.0.
    private static func clamped(
        _ gains: AVCaptureDevice.WhiteBalanceGains,
        maxGain: Float
    ) -> AVCaptureDevice.WhiteBalanceGains {
        let cap = max(1.0, maxGain)
        var value = gains
        value.redGain = min(max(value.redGain, 1.0), cap)
        value.greenGain = 1.0
        value.blueGain = min(max(value.blueGain, 1.0), cap)
        return value
    }

    // MARK: - AVCaptureVideoDataOutputSampleBufferDelegate

    func captureOutput(
        _ output: AVCaptureOutput,
        didOutput sampleBuffer: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        onSampleBuffer?(sampleBuffer)
    }
}
