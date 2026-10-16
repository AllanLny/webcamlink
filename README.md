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

**P0 — tunnel USB validé côté transport, distribution bouclée** :

1. Tunnel USB : le PC joint le listener de l'app via usbmuxd. `wcl-probe list` voit l'iPhone et
   `Connect` réussit (0 pour un port qui écoute, 3 sinon) ; le test applicatif final est
   `wcl-probe wcl-hello 5321` avec l'app lancée. Le mode `lockdown` ne sert plus que de diagnostic :
   les iOS récents exigent TLS avant toute requête lockdownd.
2. Distribution : IPA non signée buildée par GitHub Actions (workflow `build-ipa`), installée via
   AltStore/AltServer.

> Côté PC, le pilote Apple (Apple Mobile Device USB Device) est indispensable : sans lui l'iPhone
> s'attache au pilote WPD de Windows et usbmuxd ne voit rien.

**P1 — vidéo de bout en bout** :

- Côté PC : terminé et validé hors iPhone. `wcl-view` négocie la session
  (Hello/Caps/SetCamera/SetFormat/Start), décode le H.264 (Media Foundation MFT,
  `CODECAPI_AVLowLatencyMode`) et affiche l'aperçu + les stats. Chaîne validée de bout en bout
  avec `wcl-mkh264` → `wcl-fakephone` → `wcl-view` en 1280x720@30, ~8 Mbit/s, décodage ~3 ms/frame,
  chaque frame décodée correspondant exactement à sa source (tests `tests/mf_pipeline.rs`).
- Côté iOS : écrit (capture AVCaptureSession, encodage VideoToolbox low-latency sans B-frames,
  `Caps` complet, `SetCamera`/`SetFormat`/`SetOptions`/`SetImage`/`Start`/`Stop`/`RequestKeyframe`,
  `VideoConfig` + `VideoFrame` + `Status` + backpressure) — **reste à compiler via la CI macOS**
  puis à tester sur iPhone réel.

Prochaines étapes côté toi : `gh auth login` (pour pousser le repo privé et lancer la CI), brancher
l'iPhone pour `wcl-probe lockdown`, puis installer l'IPA via AltServer.

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
