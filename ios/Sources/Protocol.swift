import Foundation

enum WclMessageType: UInt8 {
    case hello = 0x01
    case caps = 0x02
    case setCamera = 0x10
    case setFormat = 0x11
    case start = 0x12
    case stop = 0x13
    case requestKeyframe = 0x14
    case setOptions = 0x15
    case setImage = 0x16
    case videoConfig = 0x20
    case videoFrame = 0x21
    case heartbeat = 0x30
    case status = 0x31
    case error = 0x7F
}

enum WclProtocol {
    static let version: UInt8 = 1
    static let magic: [UInt8] = [0x57, 0x43, 0x4C, 0x4E] // "WCLN"
    static let headerSize = 12
    static let maxPayload = 16 * 1024 * 1024
}

enum WclCodec: UInt8 {
    case h264 = 0
    case hevc = 1
}

enum WclLens: UInt8 {
    case wide = 0
    case ultraWide = 1
    case telephoto = 2
    case front = 3
    case other = 255
}

enum WclPosition: UInt8 {
    case back = 0
    case front = 1
}

struct WclCapsCodec {
    static let h264: UInt8 = 1 << 0
    static let hevc: UInt8 = 1 << 1
}

struct WclFrameFlags {
    static let keyframe: UInt8 = 1 << 0
}

enum WclErrorCode: UInt16 {
    case unsupportedVersion = 1
    case cameraPermissionDenied = 2
    case invalidFormat = 3
    case cameraUnavailable = 4
    case encoderFailure = 5
    case thermalShutdown = 6
    case notInForeground = 7
}

struct WclFrame {
    let type: WclMessageType
    var flags: UInt16 = 0
    var payload = Data()

    func encoded() -> Data {
        var out = Data(capacity: WclProtocol.headerSize + payload.count)
        out.append(contentsOf: WclProtocol.magic)
        out.append(WclProtocol.version)
        out.append(type.rawValue)
        out.append(contentsOf: withUnsafeBytes(of: flags.littleEndian) { Data($0) })
        out.append(contentsOf: withUnsafeBytes(of: UInt32(payload.count).littleEndian) { Data($0) })
        out.append(payload)
        return out
    }
}

/// Incremental WCLN frame decoder.
///
/// Deliberately uses a byte array with an explicit offset instead of `Data`
/// indices: after a `Data.removeFirst`, indices are not guaranteed to restart
/// at 0, and mixing them with `subdata(in:)` traps on the second frame.
final class WclFrameReader {
    private var buffer: [UInt8] = []

    func append(_ data: Data) -> [WclFrame] {
        buffer.append(contentsOf: data)
        var frames: [WclFrame] = []
        var offset = 0

        while buffer.count - offset >= WclProtocol.headerSize {
            let magic = WclProtocol.magic
            guard buffer[offset] == magic[0],
                  buffer[offset + 1] == magic[1],
                  buffer[offset + 2] == magic[2],
                  buffer[offset + 3] == magic[3],
                  buffer[offset + 4] == WclProtocol.version else {
                buffer.removeAll()
                return frames
            }
            let typeRaw = buffer[offset + 5]
            let flags = UInt16(buffer[offset + 6]) | (UInt16(buffer[offset + 7]) << 8)
            let length = UInt32(buffer[offset + 8])
                | (UInt32(buffer[offset + 9]) << 8)
                | (UInt32(buffer[offset + 10]) << 16)
                | (UInt32(buffer[offset + 11]) << 24)
            guard length <= UInt32(WclProtocol.maxPayload) else {
                buffer.removeAll()
                return frames
            }
            let total = WclProtocol.headerSize + Int(length)
            guard buffer.count - offset >= total else { break }
            let payload = Data(buffer[(offset + WclProtocol.headerSize)..<(offset + total)])
            offset += total
            if let type = WclMessageType(rawValue: typeRaw) {
                frames.append(WclFrame(type: type, flags: flags, payload: payload))
            }
        }

        if offset > 0 {
            buffer.removeFirst(offset)
        }
        return frames
    }
}

struct WclWriter {
    private(set) var data = Data()

    mutating func u8(_ value: UInt8) { data.append(value) }

    mutating func u16(_ value: UInt16) {
        data.append(contentsOf: withUnsafeBytes(of: value.littleEndian) { Data($0) })
    }

    mutating func u32(_ value: UInt32) {
        data.append(contentsOf: withUnsafeBytes(of: value.littleEndian) { Data($0) })
    }

    mutating func u64(_ value: UInt64) {
        data.append(contentsOf: withUnsafeBytes(of: value.littleEndian) { Data($0) })
    }

    mutating func string(_ value: String) {
        let utf8 = Data(value.utf8)
        let clamped = utf8.prefix(Int(UInt16.max))
        u16(UInt16(clamped.count))
        data.append(contentsOf: clamped)
    }

    /// `u32` longueur + octets (parameter sets H.264).
    mutating func blob(_ value: Data) {
        u32(UInt32(value.count))
        data.append(value)
    }

    /// Reste du payload, sans préfixe de longueur (données d'une frame vidéo).
    mutating func raw(_ value: Data) {
        data.append(value)
    }
}

struct WclReader {
    private let data: Data
    private var offset = 0

    init(_ data: Data) { self.data = Data(data) }

    var remaining: Int { data.count - offset }

    mutating func u8() -> UInt8? {
        guard remaining >= 1 else { return nil }
        defer { offset += 1 }
        return data[offset]
    }

    mutating func u16() -> UInt16? {
        guard remaining >= 2 else { return nil }
        defer { offset += 2 }
        return UInt16(data[offset]) | (UInt16(data[offset + 1]) << 8)
    }

    mutating func i16() -> Int16? {
        guard let raw = u16() else { return nil }
        return Int16(bitPattern: raw)
    }

    mutating func u32() -> UInt32? {
        guard remaining >= 4 else { return nil }
        defer { offset += 4 }
        return UInt32(data[offset])
            | (UInt32(data[offset + 1]) << 8)
            | (UInt32(data[offset + 2]) << 16)
            | (UInt32(data[offset + 3]) << 24)
    }

    mutating func u64() -> UInt64? {
        guard remaining >= 8 else { return nil }
        defer { offset += 8 }
        var value: UInt64 = 0
        for index in 0..<8 {
            value |= UInt64(data[offset + index]) << (8 * index)
        }
        return value
    }

    mutating func string() -> String? {
        guard let length = u16(), remaining >= Int(length) else { return nil }
        defer { offset += Int(length) }
        let slice = data.subdata(in: offset..<(offset + Int(length)))
        return String(data: slice, encoding: .utf8)
    }

    mutating func blob() -> Data? {
        guard let length = u32(), remaining >= Int(length) else { return nil }
        defer { offset += Int(length) }
        return data.subdata(in: offset..<(offset + Int(length)))
    }
}

struct WclHello {
    var minVersion: UInt8 = WclProtocol.version
    var maxVersion: UInt8 = WclProtocol.version
    var role: UInt8
    var appVersion: String
    var name: String
    var model: String

    func encoded() -> Data {
        var writer = WclWriter()
        writer.u8(minVersion)
        writer.u8(maxVersion)
        writer.u8(role)
        writer.string(appVersion)
        writer.string(name)
        writer.string(model)
        return writer.data
    }

    static func decode(_ payload: Data) -> WclHello? {
        var reader = WclReader(payload)
        guard let minVersion = reader.u8(),
              let maxVersion = reader.u8(),
              let role = reader.u8(),
              let appVersion = reader.string(),
              let name = reader.string(),
              let model = reader.string() else { return nil }
        return WclHello(
            minVersion: minVersion,
            maxVersion: maxVersion,
            role: role,
            appVersion: appVersion,
            name: name,
            model: model
        )
    }
}

// MARK: - Caps (device → PC)

struct WclCameraFormat: Equatable {
    var width: UInt16
    var height: UInt16
    var maxFps: UInt16
}

struct WclCamera {
    var cameraId: UInt8
    var position: UInt8
    var lens: UInt8
    var label: String
    var formats: [WclCameraFormat]
}

struct WclCaps {
    var cameras: [WclCamera]
    var codecs: UInt8

    func encoded() -> Data {
        var writer = WclWriter()
        writer.u8(UInt8(min(cameras.count, Int(UInt8.max))))
        for camera in cameras {
            writer.u8(camera.cameraId)
            writer.u8(camera.position)
            writer.u8(camera.lens)
            writer.string(camera.label)
            writer.u16(UInt16(min(camera.formats.count, Int(UInt16.max))))
            for format in camera.formats {
                writer.u16(format.width)
                writer.u16(format.height)
                writer.u16(format.maxFps)
            }
        }
        writer.u8(codecs)
        return writer.data
    }
}

// MARK: - PC → device

struct WclSetCamera {
    var cameraId: UInt8

    static func decode(_ payload: Data) -> WclSetCamera? {
        var reader = WclReader(payload)
        guard let cameraId = reader.u8() else { return nil }
        return WclSetCamera(cameraId: cameraId)
    }
}

struct WclSetFormat {
    var width: UInt16
    var height: UInt16
    var fps: UInt16
    var codec: UInt8
    var bitrateKbps: UInt32

    static func decode(_ payload: Data) -> WclSetFormat? {
        var reader = WclReader(payload)
        guard let width = reader.u16(),
              let height = reader.u16(),
              let fps = reader.u16(),
              let codec = reader.u8(),
              let bitrate = reader.u32() else { return nil }
        return WclSetFormat(width: width, height: height, fps: fps, codec: codec, bitrateKbps: bitrate)
    }
}

struct WclSetOptions {
    var mirror: Bool
    var rotation: UInt16
    var dimPreview: Bool

    static func decode(_ payload: Data) -> WclSetOptions? {
        var reader = WclReader(payload)
        guard let mirror = reader.u8(),
              let rotation = reader.u16(),
              let dim = reader.u8() else { return nil }
        return WclSetOptions(mirror: mirror != 0, rotation: rotation, dimPreview: dim != 0)
    }
}

/// Image controls. Every field is optional on the wire: a field that is absent
/// keeps the current value (see `docs/protocol.md`).
struct WclSetImage {
    var zoomX100: UInt16 = 100
    var exposureMode: UInt8 = 0
    var exposureBiasEvX100: Int16 = 0
    var iso: UInt16 = 0
    var exposureDurationUs: UInt32 = 0
    var focusMode: UInt8 = 0
    var focusXX10000: UInt16 = 5000
    var focusYX10000: UInt16 = 5000
    var whiteBalanceMode: UInt8 = 0
    var whiteBalanceTempK: UInt16 = 0
    var whiteBalanceTintX100: Int16 = 0
    var torch: Bool = false
    var stabilization: UInt8 = 0
    var hdr: Bool = false

    /// Applies the fields present in `payload` on top of the current state.
    func updated(with payload: Data) -> WclSetImage {
        var reader = WclReader(payload)
        var value = self
        if let zoom = reader.u16() { value.zoomX100 = zoom }
        if let mode = reader.u8() { value.exposureMode = mode }
        if let bias = reader.i16() { value.exposureBiasEvX100 = bias }
        if let iso = reader.u16() { value.iso = iso }
        if let duration = reader.u32() { value.exposureDurationUs = duration }
        if let mode = reader.u8() { value.focusMode = mode }
        if let x = reader.u16() { value.focusXX10000 = x }
        if let y = reader.u16() { value.focusYX10000 = y }
        if let mode = reader.u8() { value.whiteBalanceMode = mode }
        if let temperature = reader.u16() { value.whiteBalanceTempK = temperature }
        if let tint = reader.i16() { value.whiteBalanceTintX100 = tint }
        if let torch = reader.u8() { value.torch = torch != 0 }
        if let stabilization = reader.u8() { value.stabilization = stabilization }
        if let hdr = reader.u8() { value.hdr = hdr != 0 }
        return value
    }
}

// MARK: - device → PC

struct WclVideoConfig {
    var codec: UInt8
    var width: UInt16
    var height: UInt16
    var fps: UInt16
    var parameterSets: Data

    func encoded() -> Data {
        var writer = WclWriter()
        writer.u8(codec)
        writer.u16(width)
        writer.u16(height)
        writer.u16(fps)
        writer.blob(parameterSets)
        return writer.data
    }
}

struct WclVideoFrame {
    var ptsUs: UInt64
    var flags: UInt8
    var data: Data

    func encoded() -> Data {
        var writer = WclWriter()
        writer.u64(ptsUs)
        writer.u8(flags)
        writer.raw(data)
        return writer.data
    }
}

struct WclStatus {
    var thermalState: UInt8
    var batteryPercent: UInt8
    var charging: Bool
    var actualBitrateKbps: UInt32
    var actualFpsX100: UInt16
    var cameraId: UInt8

    func encoded() -> Data {
        var writer = WclWriter()
        writer.u8(thermalState)
        writer.u8(batteryPercent)
        writer.u8(charging ? 1 : 0)
        writer.u32(actualBitrateKbps)
        writer.u16(actualFpsX100)
        writer.u8(cameraId)
        return writer.data
    }
}

struct WclProtocolError {
    var code: WclErrorCode
    var message: String

    func encoded() -> Data {
        var writer = WclWriter()
        writer.u16(code.rawValue)
        writer.string(message)
        return writer.data
    }
}
