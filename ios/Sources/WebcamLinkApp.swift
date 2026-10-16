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
    @State private var keepScreenAwake = true

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

                Section("Écran") {
                    Toggle("Garder l'écran allumé", isOn: $keepScreenAwake)
                        .onChange(of: keepScreenAwake) { value in
                            UIApplication.shared.isIdleTimerDisabled = value
                        }
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
                server.start()
            }
        }
    }
}
