# WebcamLink

**Ton iPhone comme webcam USB sur Windows 11** — sans Mac, sans licence Apple payante, sans audio.
L'app iOS capture et encode en H.264 ; un agent PC décode le flux et alimente une **caméra virtuelle**
que voient Teams, Zoom, OBS, l'app Caméra, etc.

[![ci](https://github.com/AllanLny/webcamlink/actions/workflows/ci.yml/badge.svg)](https://github.com/AllanLny/webcamlink/actions/workflows/ci.yml)
[![build-ipa](https://github.com/AllanLny/webcamlink/actions/workflows/ipa.yml/badge.svg)](https://github.com/AllanLny/webcamlink/actions/workflows/ipa.yml)
[![Licence : MIT](https://img.shields.io/badge/licence-MIT-3B82F6.svg)](LICENSE)
![Windows 11 x64](https://img.shields.io/badge/Windows%2011-x64-0078D6.svg)

## Sommaire

- [Fonctionnement](#fonctionnement)
- [Prérequis](#prérequis)
- [Installation](#installation)
- [Réglages](#réglages)
- [Utilisation](#utilisation)
- [Dépannage](#dépannage)
- [Développement sans iPhone](#développement-sans-iphone)
- [Aperçu de l'app iOS (sans Mac)](#aperçu-de-lapp-ios-sans-mac)
- [Structure du dépôt](#structure-du-dépôt)
- [État du projet](#état-du-projet)
- [Mode automatique (iOS)](#mode-automatique-ios)
- [Contraintes iOS](#contraintes-ios)
- [Distribution de l'app iOS](#distribution-de-lapp-ios)
- [Contribuer](#contribuer)
- [Licence](#licence)

## Fonctionnement

```
iPhone (app WebcamLink)                        PC (Windows 11 x64)
  AVCaptureSession + VideoToolbox                 wcl-tray    icône + réglages
  encodage H.264                                  wcl-agent   session USB + décodage
        │                                              │
        └──── USB (usbmuxd) · protocole WCLN ──────────┘
                                                       │ mémoire partagée (NV12)
                                                       ▼
                                 Frame Server  ←  wcl-vcam.dll (source média)
                                                       │
                                                       ▼
                                         Teams, Zoom, OBS, app Caméra…
```

- Le protocole iPhone ↔ PC est décrit dans [docs/protocol.md](docs/protocol.md) ; la mémoire partagée
  agent ↔ DLL dans [docs/shm.md](docs/shm.md).
- La caméra virtuelle est créée avec `MFCreateVirtualCamera` : elle n'existe pour Windows qu'après un
  **enregistrement** (une fois, en administrateur — voir Installation).

## Prérequis

### PC

| Élément | Détail |
|---|---|
| Système | **Windows 11 x64**, build **22000** minimum (`MFCreateVirtualCamera` n'existe pas avant) |
| Droits | un shell **administrateur** une fois, pour enregistrer la caméra virtuelle |
| Exécution | **Visual C++ Redistributable 2015-2022 x64** (fourni avec VS Build Tools) |
| Pilote iPhone | **Apple Mobile Device Support** (ou l'app *Apple Devices*) : sans lui, Windows attache l'iPhone au pilote WPD et l'agent ne le voit pas |

### Compilation

| Outil | Version / composant |
|---|---|
| Visual Studio **2022 ou plus récent** | charge de travail « Développement Desktop en C++ » (MSVC v143+ + Windows SDK) — les Build Tools suffisent |
| Windows SDK | 10.0.22000 ou plus récent (le projet est compilé avec 10.0.26100) |
| CMake | ≥ 3.20 (fourni avec les Build Tools) |
| Rust | toolchain **stable**, cible MSVC (`rustup default stable-msvc`) |

### iPhone

| Élément | Détail |
|---|---|
| iOS | **16.0** ou plus récent |
| App | WebcamLink installée via AltStore/Sideloadly (Apple ID gratuit, expire après 7 jours) |
| État | app **au premier plan**, écran allumé (voir [Contraintes iOS](#contraintes-ios)) |

## Installation

1. **Compiler** (depuis la racine du dépôt) :
   ```powershell
   cargo build --release --manifest-path pc/agent/Cargo.toml
   cmake -S pc/vcam -B pc/vcam/build -A x64
   cmake --build pc/vcam/build --config Release
   ```
   CMake choisit le Visual Studio installé ; pour en forcer un (plusieurs versions côte à côte),
   ajoute par exemple `-G "Visual Studio 17 2022"`.

2. **Enregistrer la caméra virtuelle** (shell **administrateur**, une seule fois) :
   ```powershell
   pc\vcam\build\bin\Release\wcl-vcam-register.exe
   ```
   Attendu : `Caméra virtuelle "WebcamLink" enregistrée`. Pour la retirer : `--unregister` (admin).

3. **Lancer l'icône de notification** :
   ```powershell
   pc\agent\target\release\wcl-tray.exe
   ```
   Dans ses réglages, cocher **« Démarrer avec Windows »** : l'agent (c'est lui qui connecte l'iPhone)
   et l'icône se lanceront au login.
   Astuce : créer un raccourci vers `wcl-tray.exe` dans le menu Démarrer pour la retrouver par la
   recherche Windows (le MSI de P5 le fera automatiquement).

4. **Installer l'app iOS** (voir [Distribution](#distribution-de-lapp-ios)), l'ouvrir, la laisser au
   premier plan.

5. **Ouvrir la caméra** « WebcamLink (Caméra virtuelle Windows) » dans Teams, Zoom, OBS ou l'app
   Caméra. L'agent connecte l'iPhone en ~1 s.

**Mise à jour** : après un `git pull`, recompiler (étape 1). Si la caméra est déjà enregistrée, le
Frame Server peut garder l'ancienne DLL en mémoire — refaire l'étape 2 (désenregistrer, redémarrer le
service **FrameServer**, recompiler, réenregistrer) pour être sûr de tester le bon binaire.

## Réglages

Clic gauche sur l'icône (ou raccourci « WebcamLink » du menu Démarrer) → fenêtre de réglages :
résolution (**480p → 4K**), images/s (30/60), débit (**5 → 100 Mbit/s**), caméra iPhone (automatique,
arrière large / ultra grand-angle / télé, avant), miroir, démarrage avec Windows.

- **Caméra, débit, miroir** : appliqués **à chaud**, même pendant un flux — le miroir instantanément,
  caméra et débit en ~1 s (reconfiguration de la capture iPhone).
- **Résolution, images/s** : font partie du format négocié avec l'application ; ils s'appliquent au
  **prochain démarrage de Windows** (réenregistrement de la caméra).
- Les réglages sont écrits dans `%ProgramData%\WebcamLink\config.ini`, modifiable à la main :

```ini
[video]
width=1920
height=1080
fps=30
bitrate_kbps=20000
[camera]
preference=auto
mirror=0
```

| Clé | Valeurs | Défaut |
|---|---|---|
| `width` / `height` | 160–3840 / 120–2160 — 4K = plafond de la mémoire partagée | 1920 / 1080 |
| `fps` | 1–60 | 30 |
| `bitrate_kbps` | 500–100000 | 20000 |
| `preference` | `auto`, `back_wide`, `back_ultra_wide`, `back_tele`, `front` | `auto` |
| `mirror` | `0` / `1` | `0` |

> Une valeur hors bornes est ramenée dans la plage. Les commentaires ne sont pas acceptés dans les
> lignes de valeurs (la DLL lit le fichier avec les API INI de Windows).

### Réglages image

Le bouton **« Réglages image… »** (ou clic droit sur l'icône → même entrée) ouvre les contrôles « pro »
de la caméra iPhone : zoom, exposition (automatique / biais en EV / manuelle **ISO + durée**), balance
des blancs (automatique / verrouillée / manuelle en kelvins + teinte), mise au point (continu /
verrouillée à une distance / point + verrou), torche et son niveau, stabilisation, HDR.

- Les curseurs sont **bornés par ce que la caméra annonce** : l'agent écrit `caps.ini`
  (`%ProgramData%\WebcamLink\`, plage ISO, durées minimale et maximale, zoom maximal, balance, modes)
  à la connexion de l'iPhone, et la fenêtre s'en sert. Un contrôle que la caméra ne sait pas faire est
  **grisé**, tout comme ce que le mode courant rend sans effet (ISO et durée en automatique, teinte en
  balance automatique, niveau de torche quand elle est éteinte).
- **« Tout auto »** remet zoom, exposition, balance, mise au point, torche et stabilisation en
  automatique.
- Ces réglages s'appliquent **à chaud**, même pendant un flux (~250 ms : l'agent relit le fichier quatre
  fois par seconde). Hors flux, ils sont mémorisés et rejoués à la connexion suivante. Un mode refusé
  par la caméra est affiché dans la fenêtre **sans interrompre le flux**.
- Les clés correspondantes vivent dans la section `[image]` de `config.ini` (`zoom_x100`,
  `exposure_mode`, `exposure_bias_ev_x100`, `iso`, `exposure_us`, `focus_mode`, `focus_x_x10000`,
  `focus_y_x10000`, `focus_distance_x100`, `wb_mode`, `wb_temp_k`, `wb_tint_x100`, `torch`,
  `torch_level`, `stabilization`, `hdr`).
- **Applications clientes** : ces réglages sont aussi exposés à la caméra virtuelle via `IKsControl`
  (`Zoom`, `Exposure`, `Focus` en contrôle caméra ; `Gain` ↔ ISO et `WhiteBalance` ↔ température en
  proc amp). Une application qui bouge un curseur dépose sa demande dans un mapping partagé
  (`Global\WclControls`, décrit dans [docs/shm.md](docs/shm.md)) ; l'agent la reprend dans
  `config.ini`, donc elle suit exactement le même chemin que la fenêtre image. Une DLL antérieure ne
  crée pas ce mapping : la vidéo n'en est pas affectée, seuls les contrôles clients restent
  indisponibles.

## Utilisation

Menu de l'icône (clic droit) : **Réglages…**, **Démarrer/Arrêter l'agent**, **Ouvrir le journal**,
**Quitter**.

L'agent n'a rien à faire manuellement : quand une application ouvre la caméra, il connecte l'iPhone ;
il envoie `Stop` 2 s après la dernière fermeture, et réessaie chaque seconde en cas d'échec. Sans
iPhone (ou app en arrière-plan), la caméra affiche un **dégradé sombre** : c'est l'image de
remplacement, utile pour vérifier la chaîne PC → applications.

## Dépannage

| Symptôme | Cause probable | Solution |
|---|---|---|
| La caméra n'apparaît dans aucune appli | non enregistrée, shell non admin, ou Windows 10 | relancer `wcl-vcam-register.exe` en administrateur ; Windows 11 build 22000+ requis |
| Dégradé sombre au lieu de l'image | agent arrêté, iPhone absent/déverrouillé, app iOS en arrière-plan | icône → « Démarrer l'agent », brancher l'iPhone, ouvrir l'app au premier plan |
| Journal : `producteur … ≠ format annoncé` | l'iPhone ne fait pas le format choisi (ex. 4K avec la caméra avant, limitée à 1080p) | choisir un format supporté par cette caméra |
| La résolution ne change pas après réglage | format figé à l'enregistrement de la caméra | redémarrer Windows (ou réenregistrer la caméra) |
| Après une recompilation, l'ancienne DLL tourne encore | le Frame Server garde la DLL chargée | `wcl-vcam-register.exe --unregister`, redémarrer le service **FrameServer**, recompiler, réenregistrer |
| L'icône du menu Démarrer garde l'ancienne image | cache d'icônes de Windows | relancer l'Explorateur ou redémarrer |

**Journaux** :
- `%ProgramData%\WebcamLink\vcam.log` — DLL et Frame Server (un accès/erreur par ligne, avec horodatage
  et PID du processus).
- `%ProgramData%\WebcamLink\tray.log` — clics et erreurs de l'icône.
- L'agent écrit sur sa sortie standard : pour voir ses messages, lancer `wcl-agent.exe` dans un
  terminal.

## Développement sans iPhone

```powershell
cd pc/agent
cargo test                                    # protocole, session (loopback), mémoire partagée, config
cargo run --release --bin wcl-mkh264 -- test.wclc --size 1280x720 --seconds 6
cargo run --release --bin wcl-fakephone -- test.wclc          # faux iPhone, TCP 127.0.0.1:5321
cargo run --release --bin wcl-view -- --tcp 127.0.0.1:5321    # aperçu (--no-window pour les mesures)
```

Avec un iPhone branché (app lancée, écran allumé) : `wcl-view` sans `--tcp` passe par usbmuxd.

## Aperçu de l'app iOS (sans Mac)

Trois niveaux, du plus rapide au plus complet :

1. **Captures d'écran automatiques** — le workflow `preview` compile l'app pour le simulateur iOS sur un
   runner macOS, la lance et publie `apercu-clair.png` / `apercu-sombre.png` en artefacts du run. Il se
   déclenche à la demande (`Actions → preview → Run workflow`, sur n'importe quelle branche) et sur les
   PR qui touchent `ios/**` ; les images se téléchargent depuis le résumé du job.
2. **Compilation** — `ci.yml` (job `ios`) compile l'app non signée à chaque PR : c'est ce qui attrape
   les erreurs Swift, sans montrer l'écran.
3. **Sur l'iPhone** — lance `build-ipa` (`Run workflow`) en choisissant la branche, récupère l'IPA non
   signée dans les artefacts et installe-la avec Sideloadly ou AltStore : caméra et PC compris, c'est la
   vraie app.

## Structure du dépôt

```
ios/                     app iOS (SwiftUI, XcodeGen ; compilée par la CI macOS, sans Mac local)
pc/agent/                agent et outils PC (Rust)
  src/config.rs            lecture/écriture de config.ini (partagée avec la DLL)
  src/bin/wcl-agent        démon : session iPhone + décodage + mémoire partagée
  src/bin/wcl-tray         icône de notification + fenêtre de réglages
  src/bin/wcl-view         client complet (aperçu + stats)
  src/bin/wcl-probe        sonde du tunnel USB
  src/bin/wcl-mkh264       génère un clip H.264 de test (Media Foundation)
  src/bin/wcl-fakephone    rejoue ce clip comme un faux iPhone
  assets/                  logo + webcamlink.ico (embarqué dans les exécutables)
pc/vcam/                 DLL caméra virtuelle (C++/WRL, Frame Server) + outil d'enregistrement
pc/installer/            MSI WiX : agent + DLL + enregistrement + autostart — P5
pc/tools/                sonde usbmux Python
docs/                    protocole WCLN + contrat de mémoire partagée
altstore/                source AltStore (source.json)
.github/workflows/       CI : IPA non signée sur runner macOS
```

## État du projet

- **P0 — validé sur iPhone réel** : tunnel USB usbmuxd et protocole WCLN (`wcl-probe`), IPA non signée
  buildée par la CI, installation Sideloadly/AltStore.
- **P1 — validé sur iPhone réel** : vidéo de bout en bout 1080p30 (VideoToolbox low-latency, décodage
  Media Foundation). Mesures : ~20 Mbit/s, décodage 12,4 ms + rendu 7 ms (≈19 ms de pipeline).
  Correctif 0.1.1 : le listener est recréé quand l'app revient au premier plan.
- **P2 — opérationnel** : caméra virtuelle (Frame Server), mémoire partagée, agent à la demande,
  icône de notification, réglages partagés, démarrage automatique.
- **P3 / P5 — à venir** : contrôles d'image (zoom, exposition, mise au point) et MSI WiX (installation,
  enregistrement, autostart).

## Mode automatique (iOS)

L'app expose un menu **Mode automatique** :

- **Alerter quand le PC demande** — notification locale (sonore si voulu) dès qu'un client se connecte
  alors que l'app n'est plus à l'écran ; un appui dessus revient sur l'app, qui redémarre le flux. Une
  demande toutes les 30 s au maximum, pour ne pas inonder le centre de notifications.
- **Rester joignable en arrière-plan** — l'app joue un silence (mode audio de fond) pour que le
  listener survive hors écran : sans lui, iOS suspend le process et le PC ne peut plus rien demander.
- **Garder l'écran allumé** — l'écran ne s'éteint plus tant que l'app est ouverte.

Ce qu'iOS **ne permet pas**, pour aucune app : déverrouiller l'appareil ou rallumer l'écran depuis
l'extérieur, et ramener une app au premier plan toute seule. Le « déverrouillage automatique » passe
donc par la configuration du téléphone :

1. iPhone **sans code** (ou code désactivé pendant l'usage webcam) : au rallumage, l'app est déjà au
   premier plan, il n'y a rien à déverrouiller ;
2. **Verrouillage auto → Jamais** (Réglages → Affichage et luminosité) ;
3. **Accès guidé** (Réglages → Accessibilité) pour verrouiller le téléphone sur WebcamLink ;
4. **Raccourcis → Automatisation → Chargeur → « Ouvrir WebcamLink »** : l'app revient seule au premier
   plan dès que le téléphone est branché.

Le téléphone devient alors une webcam dédiée : on le branche, l'app s'ouvre, le PC se connecte et le
flux démarre sans intervention.

## Contraintes iOS

- La caméra ne fonctionne que si l'app est **au premier plan** ; écran éteint = capture impossible.
- Le PC ne peut ni déverrouiller l'iPhone ni mettre l'app au premier plan (aucune API) : il se connecte,
  l'app prévient par une notification, et le flux démarre dès que le téléphone revient.
- Mode veille : l'app reste ouverte, écran maintenu allumé → le PC démarre le flux automatiquement.
- Si l'app passe en arrière-plan, elle envoie `NotInForeground` au PC plutôt que de laisser un flux muet.

## Distribution de l'app iOS

1. Pousser un tag `v0.1.x` (ou lancer le workflow `build-ipa`) → l'IPA non signée est disponible en
   artefact de workflow / release.
2. Sur le PC : AltServer + iPhone branché → « Install .ipa » avec un Apple ID gratuit (ou Sideloadly).
3. L'app expire après 7 jours ; le rafraîchissement se fait via AltStore (AltServer en fond).

> Note distribution : le dépôt étant public, les URL de release ci-dessus sont accessibles depuis
> l'iPhone. La source AltStore du projet (`altstore/source.json`) pointe sur l'IPA de la dernière
> release — adapte le dépôt, les noms et le bundle ID si tu publies ta propre version.

## Contribuer

Les issues et les pull requests sont bienvenus. Lis d'abord [CONTRIBUTING.md](CONTRIBUTING.md) (mise en
route, attentes d'une PR, style) et le [code de conduite](CODE_OF_CONDUCT.md).

- La CI ([ci.yml](.github/workflows/ci.yml)) teste l'agent et compile la caméra virtuelle sous Windows,
  et compile l'app iOS (non signée) à chaque PR ; [build-ipa](.github/workflows/ipa.yml) construit l'IPA
  sur tag.
- Le protocole WCLN ([docs/protocol.md](docs/protocol.md)) est un contrat entre l'iOS et le PC : un
  changement de message se fait des deux côtés dans le même PR.
- Pour une vulnérabilité, n'ouvre pas d'issue : voir [SECURITY.md](SECURITY.md).

## Licence

[MIT](LICENSE) — tu peux utiliser, modifier, forker et redistribuer le projet, y compris
commercialement, à condition de conserver la mention de copyright et la licence.
