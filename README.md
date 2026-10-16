# WebcamLink

iPhone en **webcam USB** : app iOS maison + client PC léger. Projet **personnel**, sans Mac, sans
licence Apple payante (distribution **AltStore**), sans audio.

## Documentation

- [Protocole v1](docs/protocol.md) — source unique de vérité du lien iPhone ↔ PC.
- Plan de référence : `~/.commandcode/plans/webcamlink-iphone-webcam-usb.md`.

## Structure

```
ios/                  App iOS (SwiftUI, XcodeGen — pas besoin de Mac local)
pc/agent/             Agent PC (Rust) : usbmux, protocole, décodage Media Foundation, tray
pc/vcam/              DLL webcam virtuelle (C++/WRL, Frame Server)
pc/installer/         MSI (WiX) : agent + DLL + enregistrement + autostart
pc/tools/             Outils de dev (sonde usbmux, fakephone)
altstore/             Source AltStore (source.json)
.github/workflows/    CI : build IPA non signée sur runner macOS
```

## État du projet

**P0 en cours** — valider les deux risques principaux :

1. Tunnel USB : le PC joint le listener de l'app iOS via usbmuxd
   (`python pc/tools/usbmux_probe.py wcl-hello 5321`).
2. Distribution : IPA non signée buildée par GitHub Actions, installée via AltStore/AltServer.

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
