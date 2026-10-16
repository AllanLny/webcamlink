import CoreMedia
import CoreVideo
import Foundation
import VideoToolbox

/// H.264 low-latency encoder (VideoToolbox), Annex-B output.
///
/// Deliberately B-frame free (`AllowFrameReordering = false`): the PC decoder
/// runs without a reordering queue, so decode order must equal display order.
/// Keyframes carry SPS/PPS in band, and are forced at most every 2 s.
final class H264Encoder {
    private var session: VTCompressionSession?
    private let width: Int
    private let height: Int
    private let fps: Int
    private let duration: CMTime
    private var forceKeyframe = false
    private(set) var parameterSets = Data()

    /// (Annex-B access unit, pts in microseconds, keyframe)
    var onAccessUnit: ((Data, UInt64, Bool) -> Void)?
    var log: ((String) -> Void)?

    init(width: Int, height: Int, fps: Int, bitrateKbps: UInt32) throws {
        self.width = width
        self.height = height
        self.fps = max(1, fps)
        self.duration = CMTime(value: 1, timescale: CMTimeScale(max(1, fps)))

        // Low-latency rate control + hardware encoder when available.
        let specification: [String: Any] = [
            kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder as String: true,
            kVTVideoEncoderSpecification_EnableLowLatencyRateControl as String: true,
        ]

        var session: VTCompressionSession?
        let status = VTCompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            width: Int32(width),
            height: Int32(height),
            codecType: kCMVideoCodecType_H264,
            encoderSpecification: specification as CFDictionary,
            imageBufferAttributes: nil,
            compressedDataAllocator: nil,
            outputCallback: { refcon, _, status, _, sampleBuffer in
                guard status == noErr, let refcon, let sampleBuffer else { return }
                let encoder = Unmanaged<H264Encoder>.fromOpaque(refcon).takeUnretainedValue()
                encoder.handle(sampleBuffer: sampleBuffer)
            },
            refcon: Unmanaged.passUnretained(self).toOpaque(),
            compressionSessionOut: &session
        )

        guard status == noErr, let session else {
            throw CameraError.cannotConfigure("VTCompressionSessionCreate a échoué (\(status))")
        }
        self.session = session

        let bitrate = Int(bitrateKbps) * 1000
        set(kVTCompressionPropertyKey_RealTime, kCFBooleanTrue)
        set(kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse)
        set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel)
        set(kVTCompressionPropertyKey_AverageBitRate, NSNumber(value: bitrate))
        set(kVTCompressionPropertyKey_ExpectedFrameRate, NSNumber(value: self.fps))
        set(kVTCompressionPropertyKey_MaxKeyFrameInterval, NSNumber(value: self.fps * 2))
        set(kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, NSNumber(value: 2))
        VTCompressionSessionPrepareToEncodeFrames(session)
    }

    private func set(_ key: CFString, _ value: CFTypeRef) {
        guard let session else { return }
        let status = VTSessionSetProperty(session, key: key, value: value)
        if status != noErr {
            log?("VideoToolbox: propriété refusée (\(status))")
        }
    }

    func requestKeyframe() {
        forceKeyframe = true
    }

    func encode(pixelBuffer: CVPixelBuffer, presentationTime: CMTime) {
        guard let session else { return }
        var properties: CFDictionary?
        if forceKeyframe {
            forceKeyframe = false
            properties = [kVTEncodeFrameOptionKey_ForceKeyFrame: true] as CFDictionary
        }
        let status = VTCompressionSessionEncodeFrame(
            session,
            imageBuffer: pixelBuffer,
            presentationTimeStamp: presentationTime,
            duration: duration,
            frameProperties: properties,
            sourceFrameRefCon: nil,
            infoFlagsOut: nil
        )
        if status != noErr {
            log?("VTCompressionSessionEncodeFrame: \(status)")
        }
    }

    func finish() {
        guard let session else { return }
        VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
        VTCompressionSessionInvalidate(session)
        self.session = nil
    }

    // MARK: - Output

    private func handle(sampleBuffer: CMSampleBuffer) {
        guard CMSampleBufferDataIsReady(sampleBuffer) else { return }
        guard let formatDescription = CMSampleBufferGetFormatDescription(sampleBuffer) else { return }
        guard let dataBuffer = CMSampleBufferGetDataBuffer(sampleBuffer) else { return }

        let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[CFString: Any]]
        let notSync = attachments?.first?[kCMSampleAttachmentKey_NotSync] as? Bool ?? false
        let isKeyframe = !notSync

        var annexB = Data()
        if isKeyframe {
            let sets = Self.parameterSets(from: formatDescription)
            if !sets.isEmpty {
                parameterSets = sets
            }
            annexB.append(parameterSets)
        }

        var totalLength = 0
        var pointer: UnsafeMutablePointer<Int8>?
        let status = CMBlockBufferGetDataPointer(
            dataBuffer,
            atOffset: 0,
            lengthAtOffsetOut: nil,
            totalLengthOut: &totalLength,
            dataPointerOut: &pointer
        )
        guard status == kCMBlockBufferNoErr, let pointer else { return }

        let bytes = UnsafeRawPointer(pointer).assumingMemoryBound(to: UInt8.self)
        let headerLength = max(1, Self.nalUnitHeaderLength(from: formatDescription))
        var offset = 0
        while offset + headerLength <= totalLength {
            var nalSize = 0
            for index in 0..<headerLength {
                nalSize = (nalSize << 8) | Int(bytes[offset + index])
            }
            offset += headerLength
            guard nalSize > 0, offset + nalSize <= totalLength else { break }
            // AVCC → Annex-B: 4-byte length replaced by a start code.
            annexB.append(contentsOf: [0x00, 0x00, 0x00, 0x01])
            annexB.append(Data(bytes: bytes + offset, count: nalSize))
            offset += nalSize
        }

        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        let seconds = CMTimeGetSeconds(pts)
        let ptsUs = seconds.isFinite && seconds > 0 ? UInt64(seconds * 1_000_000) : 0
        onAccessUnit?(annexB, ptsUs, isKeyframe)
    }

    private static func parameterSets(from formatDescription: CMFormatDescription) -> Data {
        var sets = Data()
        var count = 0
        var nalLength: Int32 = 0
        let probe = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            formatDescription,
            parameterSetIndex: 0,
            parameterSetPointerOut: nil,
            parameterSetSizeOut: nil,
            parameterSetCountOut: &count,
            nalUnitHeaderLengthOut: &nalLength
        )
        guard probe == noErr, count > 0 else { return sets }

        for index in 0..<count {
            var pointer: UnsafePointer<UInt8>?
            var size = 0
            let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                formatDescription,
                parameterSetIndex: index,
                parameterSetPointerOut: &pointer,
                parameterSetSizeOut: &size,
                parameterSetCountOut: nil,
                nalUnitHeaderLengthOut: nil
            )
            guard status == noErr, let pointer, size > 0 else { continue }
            sets.append(contentsOf: [0x00, 0x00, 0x00, 0x01])
            sets.append(Data(bytes: pointer, count: size))
        }
        return sets
    }

    private static func nalUnitHeaderLength(from formatDescription: CMFormatDescription) -> Int {
        var count = 0
        var nalLength: Int32 = 4
        let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            formatDescription,
            parameterSetIndex: 0,
            parameterSetPointerOut: nil,
            parameterSetSizeOut: nil,
            parameterSetCountOut: &count,
            nalUnitHeaderLengthOut: &nalLength
        )
        return status == noErr ? Int(nalLength) : 4
    }
}
