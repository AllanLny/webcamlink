import AVFoundation
import Foundation

/// Keeps the process alive once the app leaves the screen, so the listener
/// stays open and the PC can still reach us — which is what makes the "the PC
/// asked for the camera" notification possible. iOS only grants that time to
/// apps that are doing something audible, so we play silence.
final class AudioKeepAlive {
    static let shared = AudioKeepAlive()

    private var engine: AVAudioEngine?
    private var player: AVAudioPlayerNode?

    private init() {}

    var isRunning: Bool { engine?.isRunning ?? false }

    func start() {
        guard !isRunning else { return }
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playback, mode: .default, options: [.mixWithOthers])
            try session.setActive(true)

            guard let format = AVAudioFormat(standardFormatWithSampleRate: 44_100, channels: 1) else {
                LinkServer.shared.log("Veille: format audio indisponible")
                return
            }
            guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 44_100) else {
                LinkServer.shared.log("Veille: tampon audio indisponible")
                return
            }

            let engine = AVAudioEngine()
            let player = AVAudioPlayerNode()
            engine.attach(player)
            engine.connect(player, to: engine.mainMixerNode, format: format)
            engine.mainMixerNode.outputVolume = 0

            buffer.frameLength = 44_100  // one second of silence, looped forever
            try engine.start()
            player.scheduleBuffer(buffer, at: nil, options: [.loops], completionHandler: nil)
            player.play()

            self.engine = engine
            self.player = player
            LinkServer.shared.log("Veille: maintien en arrière-plan actif (silence)")
        } catch {
            LinkServer.shared.log("Veille: maintien en arrière-plan impossible — \(error.localizedDescription)")
        }
    }

    func stop() {
        guard let engine else { return }
        player?.stop()
        engine.stop()
        player = nil
        self.engine = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: [.notifyOthersOnDeactivation])
        LinkServer.shared.log("Veille: maintien en arrière-plan désactivé")
    }
}
