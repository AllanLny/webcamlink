import AVFoundation
import CoreMedia
import Foundation

/// One camera exposed to the PC; mirrors the `Caps` message.
struct WclCameraDescriptor {
    var cameraId: UInt8
    var position: UInt8
    var lens: UInt8
    var label: String
    var formats: [WclCameraFormat]
    var device: AVCaptureDevice
}

enum CameraError: Error, CustomStringConvertible {
    case permissionDenied
    case noSuchCamera
    case noSuchFormat(String)
    case cannotConfigure(String)

    var description: String {
        switch self {
        case .permissionDenied: return "accès caméra refusé"
        case .noSuchCamera: return "caméra inconnue"
        case .noSuchFormat(let detail): return "format indisponible (\(detail))"
        case .cannotConfigure(let detail): return "configuration refusée (\(detail))"
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
                    device: device
                )
            )
        }
        return descriptors
    }

    // MARK: - Configuration

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
        if session.isRunning {
            session.stopRunning()
        }
        isRunning = false
        log?("Capture arrêtée")
    }

    // MARK: - Image controls (P1: zoom + torche; le reste en P3)

    func apply(image: WclSetImage) {
        guard let device = descriptor?.device else { return }
        do {
            try device.lockForConfiguration()
        } catch {
            log?("Réglages image refusés: \(error.localizedDescription)")
            return
        }
        if device.hasTorch && device.isTorchAvailable {
            device.torchMode = image.torch ? .on : .off
        }
        let zoom = max(1.0, min(Double(image.zoomX100) / 100.0, device.activeFormat.videoMaxZoomFactor))
        device.videoZoomFactor = zoom
        device.unlockForConfiguration()
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
