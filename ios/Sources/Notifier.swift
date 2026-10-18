import Foundation
import UserNotifications

/// Local notifications. When the app is not on screen they are the only way the
/// PC's request can reach the user: iOS gives an app no API to wake the screen,
/// unlock the device or bring itself back to the foreground.
enum Notifier {
    /// Keys shared with the app's own switches through `@AppStorage`.
    static let alertKey = "auto.notifyCameraRequest"
    static let soundKey = "auto.notifySound"

    /// At most one notification per window: the PC retries its connection every
    /// second, and the Notification Center must not turn into a flood.
    private static let minimumInterval: TimeInterval = 30
    private static var lastPosted = Date.distantPast

    /// Asks for the permission the first time the switch is turned on.
    static func requestAuthorizationIfUndetermined() {
        UNUserNotificationCenter.current().getNotificationSettings { settings in
            guard settings.authorizationStatus == .notDetermined else { return }
            UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) {
                granted, error in
                if let error {
                    LinkServer.shared.log("Notifications: erreur — \(error.localizedDescription)")
                } else {
                    LinkServer.shared.log("Notifications: autorisation \(granted ? "accordée" : "refusée")")
                }
            }
        }
    }

    /// "The PC is asking for the camera" — posted when a client connects while
    /// the app is not in the foreground.
    static func postCameraRequest() {
        DispatchQueue.main.async {
            let defaults = UserDefaults.standard
            guard (defaults.object(forKey: alertKey) as? Bool) ?? true else { return }
            guard Date().timeIntervalSince(lastPosted) >= minimumInterval else { return }
            lastPosted = Date()

            let content = UNMutableNotificationContent()
            content.title = "Le PC demande la caméra"
            content.body = "Touche pour revenir sur WebcamLink : le flux redémarre tout seul."
            content.sound = ((defaults.object(forKey: soundKey) as? Bool) ?? true)
                ? UNNotificationSound.default
                : nil

            let request = UNNotificationRequest(
                identifier: "wcl-camera-request",
                content: content,
                trigger: nil
            )
            UNUserNotificationCenter.current().add(request) { error in
                if let error {
                    LinkServer.shared.log("Notification impossible: \(error.localizedDescription)")
                } else {
                    LinkServer.shared.log("Notification « le PC demande la caméra » envoyée")
                }
            }
        }
    }
}
