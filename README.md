# WebcamLink

iPhone en **webcam USB** : app iOS maison + client PC léger. Projet **personnel**, sans Mac, sans
licence Apple payante (distribution **AltStore**), sans audio.

## Documentation

- [Protocole v1](docs/protocol.md) — source unique de vérité du lien iPhone ↔ PC.
- Plan de référence : `~/.commandcode/plans/webcamlink-iphone-webcam-usb.md`.

## Structure

```
ios/                  App iOS (SwiftUI, XcodeGen — pas besoin de Mac local)
pc/agent/             Agent PC (Rust) : usbmux, protocole, décodage Media Foundation, aperçu
  src/bin/wcl-probe     outil de validation du tunnel USB (P0)
  src/bin/wcl-mkh264    génère un clip H.264 de test (Media Foundation, sans ffmpeg)
  src/bin/wcl-fakephone rejoue ce clip comme un faux iPhone, en WCLN sur TCP
  src/bin/wcl-view      client PC complet : session + décodage + aperçu + stats
pc/vcam/              DLL webcam virtuelle (C++/WRL, Frame Server)          — P2
pc/installer/         MSI (WiX) : agent + DLL + enregistrement + autostart   — P5
pc/tools/             Sonde usbmux Python (P0)
altstore/             Source AltStore (source.json)
.github/workflows/    CI : build IPA non signée sur runner macOS
```

## État du projet

**P0 — terminé et validé sur iPhone réel** :

1. Tunnel USB : `wcl-probe wcl-hello 5321` reçoit le `Hello` de l'app et l'écho du `Heartbeat`
   (« TUNNEL USB OK — round-trip WCLN complet validé »). Le mode `lockdown` ne sert plus que de
   diagnostic : les iOS récents exigent TLS avant toute requête lockdownd.
2. Distribution : IPA non signée buildée par GitHub Actions (workflow `build-ipa`), installée avec
   **Sideloadly** (signature avec un Apple ID gratuit ; l'app est réenrôlée pour l'auto-refresh).
   Sideloadly mange le bundle ID (`com.allan.webcamlink.L5APTZGU7R`, `ALTBundleIdentifier` = l'original).

> Côté PC, le pilote Apple (Apple Mobile Device USB Device) est indispensable : sans lui l'iPhone
> s'attache au pilote WPD de Windows et usbmuxd ne voit rien. Sideloadly a aussi besoin d'**Apple
> Application Support** (`winget install Apple.AppleApplicationSupport.x64`) et refuse les IP
> bloquées par Apple pour GSA (503) — un partage de connexion iPhone contourne le blocage.

**P1 — vidéo de bout en bout, validée sur iPhone réel (1920x1080 @ 30 fps)** :

- iOS : capture AVCaptureSession, encodage VideoToolbox low-latency sans B-frames, `Caps` complet,
  `SetCamera`/`SetFormat`/`SetOptions`/`Start`/`Stop`/`RequestKeyframe`, `VideoConfig` +
  `VideoFrame` + `Status`, backpressure.
- PC : `wcl-view` négocie la session, décode le H.264 (Media Foundation, `CODECAPI_AVLowLatencyMode`)
  et affiche l'aperçu + les stats.
- Mesures réelles : **~20 Mbit/s**, décodage **12,4 ms** + rendu **7 ms** (≈19 ms de pipeline).
  La chaîne offline (`wcl-mkh264` → `wcl-fakephone` → `wcl-view`) reste la base des tests sans iPhone.
- Limite connue : le listener de l'app **meurt quand iOS la suspend** (arrière-plan/écran verrouillé)
  et n'est pas encore relancé au retour au premier plan — relancer l'app suffit en attendant (P4).

**Prochaine étape : P2 — webcam virtuelle** (DLL C++/WRL + Frame Server + mémoire partagée) pour que
l'iPhone apparaisse comme une caméra système dans Teams/Zoom/OBS/Chrome.

## Développement sans iPhone

```powershell
cd pc/agent
cargo test                                   # protocole, session (loopback), Media Foundation
cargo run --release --bin wcl-mkh264 -- test.wclc --size 1280x720 --seconds 6
cargo run --release --bin wcl-fakephone -- test.wclc          # écoute sur 127.0.0.1:5321
cargo run --release --bin wcl-view -- --tcp 127.0.0.1:5321    # aperçu (ajouter --no-window pour les mesures)
```

Avec un iPhone branché (app lancée, écran allumé) : `wcl-view` sans `--tcp` passe par usbmuxd.

## Build & installation (P0)

1. Pousser un tag `v0.1.0` (ou lancer le workflow `build-ipa` manuellement) → l'IPA non signée est
   disponible en artefact de workflow / release.
2. Sur le PC : AltServer (tray) + iPhone branché → menu AltServer → « Install .ipa » avec ton Apple ID
   gratuit.
3. L'app s'installe, expire après 7 jours ; le refresh se fait via AltStore (+ AltServer en fond).

> Note distribution : tant que le repo est privé, les URL GitHub ne sont pas accessibles depuis
> l'iPhone. P5 tranchera entre un repo public de distribution (IPA + source.json uniquement) ou une
> installation manuelle via AltServer.

## Contraintes iOS (importantes)

- La caméra ne fonctionne que si l'app est **au premier plan** ; écran verrouillé = capture impossible.
- Le PC ne peut ni déverrouiller l'iPhone ni mettre l'app au premier plan (aucune API).
- Mode veille : l'app reste ouverte, écran maintenu allumé, aperçu assombri → le PC démarre le flux
  automatiquement.
- Alerte de secours : si l'app est en arrière-plan (gardée vivante), une notification locale + son
  prévient quand le PC demande la caméra.
