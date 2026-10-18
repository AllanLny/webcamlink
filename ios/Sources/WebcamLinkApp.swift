import SwiftUI
import UIKit

@main
struct WebcamLinkApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

struct ContentView: View {
    @ObservedObject private var server = LinkServer.shared
    @AppStorage("screen.keepAwake") private var keepScreenAwake = true
    @AppStorage(Notifier.alertKey) private var notifyCameraRequest = true
    @AppStorage(Notifier.soundKey) private var notifyWithSound = true
    @AppStorage("auto.keepReachable") private var keepReachable = true

    var body: some View {
        NavigationStack {
            List {
                Section("État") {
                    HStack {
                        Text("Écoute")
                        Spacer()
                        Text(server.isListening ? "active" : "arrêtée")
                            .foregroundColor(server.isListening ? .green : .red)
                    }
                    HStack {
                        Text("Endpoint")
                        Spacer()
                        Text(server.listenEndpoint.isEmpty ? "—" : server.listenEndpoint)
                            .foregroundColor(.secondary)
                    }
                    HStack {
                        Text("Clients connectés")
                        Spacer()
                        Text("\(server.connectedClients)")
                            .foregroundColor(.secondary)
                    }
                    HStack {
                        Text("Flux")
                        Spacer()
                        Text(server.streaming ? "en cours" : "à l'arrêt")
                            .foregroundColor(server.streaming ? .green : .secondary)
                    }
                    HStack {
                        Text("iPhone")
                        Spacer()
                        Text(server.statusSummary)
                            .foregroundColor(.secondary)
                            .multilineTextAlignment(.trailing)
                    }
                    if let error = server.lastError {
                        Text(error).foregroundColor(.red)
                    }
                }

                Section("Caméras") {
                    Text(server.cameraSummary)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(.secondary)
                }

                Section {
                    Toggle("Alerter quand le PC demande", isOn: $notifyCameraRequest)
                        .onChange(of: notifyCameraRequest) { enabled in
                            if enabled { Notifier.requestAuthorizationIfUndetermined() }
                        }
                    Toggle("Notification sonore", isOn: $notifyWithSound)
                        .disabled(!notifyCameraRequest)
                    Toggle("Rester joignable en arrière-plan", isOn: $keepReachable)
                        .onChange(of: keepReachable) { enabled in
                            if enabled {
                                AudioKeepAlive.shared.start()
                            } else {
                                AudioKeepAlive.shared.stop()
                            }
                        }
                    Toggle("Garder l'écran allumé", isOn: $keepScreenAwake)
                        .onChange(of: keepScreenAwake) { value in
                            UIApplication.shared.isIdleTimerDisabled = value
                        }
                } header: {
                    Text("Mode automatique")
                } footer: {
                    hint(
                        "Le PC se connecte tout seul dès qu'une application ouvre la caméra. "
                        + "Ces options font le reste : notification à chaque demande quand l'app n'est plus à "
                        + "l'écran, et maintien de l'écoute en arrière-plan (l'app joue un silence, seule façon "
                        + "d'y être autorisé par iOS)."
                    )
                }

                Section {
                    hint(
                        "Une app iOS ne peut ni déverrouiller l'iPhone ni rallumer l'écran : si l'écran "
                        + "s'éteint, le flux s'arrête. Aucun logiciel ne peut contourner ça."
                    )
                    hint(
                        "Sans code, l'app est déjà au premier plan au rallumage : il n'y a rien à "
                        + "déverrouiller, un appui sur le bouton latéral suffit."
                    )
                    hint("Verrouillage auto : Réglages → Affichage et luminosité → Verrouillage auto → Jamais.")
                    hint(
                        "Kiosque : Réglages → Accessibilité → Accès guidé, puis triple clic sur le bouton "
                        + "latéral quand WebcamLink est ouvert."
                    )
                    hint(
                        "Ouverture automatique : app Raccourcis → Automatisation → Chargeur → « Ouvrir "
                        + "WebcamLink ». Dès que le téléphone est branché, l'app revient au premier plan."
                    )
                } header: {
                    Text("Aller plus loin")
                } footer: {
                    hint(
                        "Branche l'iPhone, ouvre WebcamLink, active le mode automatique : le téléphone devient "
                        + "une webcam prête à l'emploi. Le PC reste maître du flux."
                    )
                }

                Section("Journal") {
                    if server.logLines.isEmpty {
                        Text("En attente…").foregroundColor(.secondary)
                    } else {
                        ForEach(Array(server.logLines.enumerated()), id: \.offset) { _, line in
                            Text(line)
                                .font(.system(.caption, design: .monospaced))
                        }
                    }
                }
            }
            .navigationTitle("WebcamLink")
            .onAppear {
                UIApplication.shared.isIdleTimerDisabled = keepScreenAwake
                if notifyCameraRequest { Notifier.requestAuthorizationIfUndetermined() }
                if keepReachable { AudioKeepAlive.shared.start() }
                server.start()
            }
        }
    }

    private func hint(_ text: String) -> some View {
        Text(text)
            .font(.footnote)
            .foregroundColor(.secondary)
    }
}
