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

final class WclFrameReader {
    private var buffer = Data()

    func append(_ data: Data) -> [WclFrame] {
        buffer.append(data)
        var frames: [WclFrame] = []
        while buffer.count >= WclProtocol.headerSize {
            let header = [UInt8](buffer.prefix(WclProtocol.headerSize))
            guard Array(header[0..<4]) == WclProtocol.magic, header[4] == WclProtocol.version else {
                buffer.removeAll()
                break
            }
            let typeRaw = header[5]
            let flags = UInt16(header[6]) | (UInt16(header[7]) << 8)
            let length = UInt32(header[8])
                | (UInt32(header[9]) << 8)
                | (UInt32(header[10]) << 16)
                | (UInt32(header[11]) << 24)
            guard length <= UInt32(WclProtocol.maxPayload) else {
                buffer.removeAll()
                break
            }
            let total = WclProtocol.headerSize + Int(length)
            guard buffer.count >= total else { break }
            let payload = buffer.subdata(in: WclProtocol.headerSize..<total)
            buffer.removeFirst(total)
            if let type = WclMessageType(rawValue: typeRaw) {
                frames.append(WclFrame(type: type, flags: flags, payload: payload))
            }
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
