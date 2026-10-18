import AVFoundation
import Darwin
import Foundation
import Network
import UIKit

/// TCP server: listens on 127.0.0.1:5321 (reachable from the PC through
/// usbmuxd), speaks the WCLN framing and drives one `LinkSession` at a time —
/// a new connection replaces the previous one.
final class LinkServer: ObservableObject {
    static let shared = LinkServer()
    static let port: UInt16 = 5321

    @Published private(set) var isListening = false
    @Published private(set) var listenEndpoint = ""
    @Published private(set) var connectedClients = 0
    @Published private(set) var lastError: String?
    @Published private(set) var logLines: [String] = []
    @Published private(set) var streaming = false
    @Published private(set) var cameraSummary = "—"
    @Published private(set) var statusSummary = "—"

    private let queue = DispatchQueue(label: "com.allan.webcamlink.server")
    private var listener: NWListener?
    private var connection: NWConnection?
    private var pendingBytes = 0
    private var session: LinkSession?
    private var cameras: [WclCameraDescriptor] = []
    private var usedFallbackBinding = false

    /// Log file in Documents — survives a crash, unlike the in-app journal.
    private let logFileURL: URL? = {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first?
            .appendingPathComponent("webcamlink.log")
    }()

    private init() {}

    // MARK: - Lifecycle

    func start() {
        queue.async { self.startOnQueue() }
    }

    func stop() {
        queue.async {
            self.session?.close()
            self.session = nil
            self.connection?.cancel()
            self.connection = nil
            self.listener?.cancel()
            self.listener = nil
            self.publish {
                self.connectedClients = 0
                self.streaming = false
            }
        }
    }

    private func startOnQueue() {
        guard listener == nil else { return }

        if AVCaptureDevice.authorizationStatus(for: .video) == .notDetermined {
            AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
                self?.log("Accès caméra: \(granted ? "accordé" : "refusé")")
                self?.queue.async { self?.refreshCameras() }
            }
        }

        refreshCameras()

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

    private func refreshCameras() {
        cameras = CameraController.discover()
        publish {
            self.cameraSummary = self.cameras.isEmpty
                ? "aucune caméra (autorisation ?)"
                : self.cameras.map { "\($0.label): \($0.formats.count) format(s)" }.joined(separator: " · ")
        }
        log("Caméras détectées: \(cameras.count)")
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
        // One client at a time: the new connection replaces the previous one.
        self.session?.close()
        self.session = nil
        self.connection?.cancel()
        self.connection = connection
        self.pendingBytes = 0

        publish { self.connectedClients = 1 }
        log("Client connecté")

        let session = LinkSession(
            queue: queue,
            cameras: cameras,
            send: { [weak self] frame in self?.send(frame) },
            pendingBytes: { [weak self] in self?.pendingBytes ?? 0 },
            log: { [weak self] message in self?.log(message) }
        )
        session.onStreamingChanged = { [weak self] streaming in
            self?.publish { self?.streaming = streaming }
        }
        session.onStatusText = { [weak self] text in
            self?.publish { self?.statusSummary = text }
        }
        self.session = session

        let reader = WclFrameReader()

        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                self.log("Canal prêt")
                self.session?.sendHello()
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
            connection.receive(minimumIncompleteLength: 1, maximumLength: 256 * 1024) {
                [weak self] data, _, isComplete, error in
                if let data, !data.isEmpty {
                    for frame in reader.append(data) {
                        self?.session?.handle(frame: frame)
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
        guard self.connection === connection else { return }
        session?.close()
        session = nil
        self.connection = nil
        pendingBytes = 0
        publish {
            self.connectedClients = 0
            self.streaming = false
            self.statusSummary = "—"
        }
    }

    private func send(_ frame: WclFrame) {
        guard let connection else { return }
        let data = frame.encoded()
        pendingBytes += data.count
        connection.send(content: data, completion: .contentProcessed { [weak self] error in
            guard let self else { return }
            if let error {
                self.log("Envoi impossible: \(error.localizedDescription)")
            }
            self.pendingBytes -= data.count
        })
    }

    // MARK: - Utilities

    func log(_ message: String) {
        // A DateFormatter is not thread-safe: create one per call (cheap at our rate).
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss.SSS"
        let stamp = formatter.string(from: Date())
        let line = "[\(stamp)] \(message)"
        appendToLogFile(line)
        publish {
            self.logLines.append(line)
            if self.logLines.count > 400 {
                self.logLines.removeFirst(self.logLines.count - 400)
            }
        }
    }

    private func appendToLogFile(_ line: String) {
        guard let url = logFileURL else { return }
        let data = Data((line + "\n").utf8)
        let size = (try? FileManager.default.attributesOfItem(atPath: url.path))?[.size] as? Int ?? 0
        if size > 200_000 {
            try? FileManager.default.removeItem(at: url)
        }
        if let handle = try? FileHandle(forWritingTo: url) {
            handle.seekToEndOfFile()
            handle.write(data)
            try? handle.close()
        } else {
            try? data.write(to: url)
        }
    }

    private func publish(_ work: @escaping () -> Void) {
        if Thread.isMainThread {
            work()
        } else {
            DispatchQueue.main.async(execute: work)
        }
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
}
