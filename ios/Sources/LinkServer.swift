import Darwin
import Foundation
import Network
import UIKit

/// P0 « hello » server: listens on 127.0.0.1:5321 (reachable from the PC through
/// usbmuxd), speaks the WCLN framing, answers Hello and echoes Heartbeats.
final class LinkServer: ObservableObject {
    static let shared = LinkServer()
    static let port: UInt16 = 5321

    @Published private(set) var isListening = false
    @Published private(set) var listenEndpoint = ""
    @Published private(set) var connectedClients = 0
    @Published private(set) var lastError: String?
    @Published private(set) var logLines: [String] = []

    private let queue = DispatchQueue(label: "com.allan.webcamlink.server")
    private var listener: NWListener?
    private var connections: [ObjectIdentifier: NWConnection] = [:]
    private var usedFallbackBinding = false

    private static let timeFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss.SSS"
        return formatter
    }()

    private init() {}

    // MARK: - Lifecycle

    func start() {
        queue.async { self.startOnQueue() }
    }

    func stop() {
        queue.async {
            self.listener?.cancel()
            self.listener = nil
            for connection in self.connections.values {
                connection.cancel()
            }
            self.connections.removeAll()
            self.publish {
                self.connectedClients = 0
            }
        }
    }

    private func startOnQueue() {
        guard listener == nil else { return }
        do {
            let parameters = NWParameters.tcp
            parameters.allowLocalEndpointReuse = true
            if let port = NWEndpoint.Port(rawValue: Self.port) {
                parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: port)
            }
            let listener = try NWListener(using: parameters)
            attach(listener)
            listener.start(queue: queue)
            log("Listener en démarrage sur 127.0.0.1:\(Self.port)…")
        } catch {
            log("ERREUR: échec de création du listener — \(error.localizedDescription)")
            publish { self.lastError = error.localizedDescription }
        }
    }

    private func attach(_ listener: NWListener) {
        self.listener = listener

        listener.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                self.publish {
                    self.isListening = true
                    self.listenEndpoint = "127.0.0.1:\(Self.port)"
                }
                self.log("Écoute active sur 127.0.0.1:\(Self.port)")
            case .failed(let error):
                self.publish {
                    self.isListening = false
                    self.lastError = error.localizedDescription
                }
                self.log("Listener en échec: \(error.localizedDescription)")
                self.listener = nil
                if !self.usedFallbackBinding {
                    self.usedFallbackBinding = true
                    self.log("Nouvel essai sans binding loopback explicite…")
                    self.restartWithDefaultBinding()
                }
            case .cancelled:
                self.publish { self.isListening = false }
                self.log("Listener arrêté")
                self.listener = nil
            default:
                break
            }
        }

        listener.newConnectionHandler = { [weak self] connection in
            self?.handle(connection)
        }
    }

    private func restartWithDefaultBinding() {
        do {
            let parameters = NWParameters.tcp
            parameters.allowLocalEndpointReuse = true
            guard let port = NWEndpoint.Port(rawValue: Self.port) else { return }
            let listener = try NWListener(using: parameters, on: port)
            attach(listener)
            listener.start(queue: queue)
        } catch {
            log("ERREUR fallback: \(error.localizedDescription)")
        }
    }

    // MARK: - Connections

    private func handle(_ connection: NWConnection) {
        let id = ObjectIdentifier(connection)
        connections[id] = connection
        publish { self.connectedClients = self.connections.count }
        log("Client connecté (\(connections.count) actif(s))")

        let reader = WclFrameReader()

        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                self.log("Canal prêt")
                self.send(self.makeHelloFrame(), on: connection)
            case .failed(let error):
                self.log("Client en échec: \(error.localizedDescription)")
                connection.cancel()
            case .waiting(let error):
                self.log("Client en attente: \(error.localizedDescription)")
            case .cancelled:
                self.log("Client déconnecté")
                self.remove(connection)
            default:
                break
            }
        }

        func receiveLoop() {
            connection.receive(minimumIncompleteLength: 1, maximumLength: 256 * 1024) { [weak self] data, _, isComplete, error in
                if let data, !data.isEmpty {
                    let frames = reader.append(data)
                    for frame in frames {
                        self?.handle(frame: frame, on: connection)
                    }
                }
                if let error {
                    self?.log("Erreur de réception: \(error.localizedDescription)")
                    connection.cancel()
                    return
                }
                if isComplete {
                    self?.log("Flux terminé par le client")
                    connection.cancel()
                    return
                }
                receiveLoop()
            }
        }

        connection.start(queue: queue)
        receiveLoop()
    }

    private func remove(_ connection: NWConnection) {
        connections.removeValue(forKey: ObjectIdentifier(connection))
        publish { self.connectedClients = self.connections.count }
    }

    private func send(_ frame: WclFrame, on connection: NWConnection) {
        connection.send(content: frame.encoded(), completion: .contentProcessed { [weak self] error in
            if let error {
                self?.log("Envoi impossible: \(error.localizedDescription)")
            }
        })
    }

    // MARK: - Protocol handling (P0: Hello + Heartbeat)

    private func handle(frame: WclFrame, on connection: NWConnection) {
        switch frame.type {
        case .hello:
            if let hello = WclHello.decode(frame.payload) {
                log("Hello reçu — role=\(hello.role) app=\(hello.appVersion) name=« \(hello.name) » model=\(hello.model) v\(hello.minVersion)–\(hello.maxVersion)")
            } else {
                log("Hello illisible (\(frame.payload.count) octets)")
            }
            send(makeHelloFrame(), on: connection)

        case .heartbeat:
            var reader = WclReader(frame.payload)
            var writer = WclWriter()
            writer.u64(reader.u64() ?? 0)
            send(WclFrame(type: .heartbeat, payload: writer.data), on: connection)
            log("Heartbeat écho")

        default:
            log("Message \(frame.type) reçu (\(frame.payload.count) octets) — ignoré en P0")
        }
    }

    private func makeHelloFrame() -> WclFrame {
        let hello = WclHello(
            role: 1,
            appVersion: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0",
            name: UIDevice.current.name,
            model: Self.deviceModel()
        )
        return WclFrame(type: .hello, payload: hello.encoded())
    }

    static func deviceModel() -> String {
        var systemInfo = utsname()
        uname(&systemInfo)
        let mirror = Mirror(reflecting: systemInfo.machine)
        var identifier = ""
        for child in mirror.children {
            guard let value = child.value as? Int8, value != 0 else { break }
            identifier.append(String(UnicodeScalar(UInt8(bitPattern: value))))
        }
        return identifier.isEmpty ? "iOS" : identifier
    }

    // MARK: - Utilities

    func log(_ message: String) {
        let stamp = Self.timeFormatter.string(from: Date())
        publish {
            self.logLines.append("[\(stamp)] \(message)")
            if self.logLines.count > 400 {
                self.logLines.removeFirst(self.logLines.count - 400)
            }
        }
    }

    private func publish(_ work: @escaping () -> Void) {
        if Thread.isMainThread {
            work()
        } else {
            DispatchQueue.main.async(execute: work)
        }
    }
}
