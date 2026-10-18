# Contribuer à WebcamLink

Merci ! Ce projet est un outil personnel, mais les correctifs, les idées et les rapports de bugs sont
les bienvenus. Ce guide résume ce qu'il faut savoir pour qu'une contribution parte du bon pied.

## Avant de commencer

- **Windows 11 x64** (build 22000+) avec les prérequis du [README](README.md#prérequis) : VS Build
  Tools 2022 (charge C++), CMake ≥ 3.20, Rust stable (cible MSVC).
- Pour tester de bout en bout : un iPhone (iOS 16+) avec l'app installée via AltStore/Sideloadly.
- Sans iPhone, la chaîne offline `wcl-mkh264` → `wcl-fakephone` → `wcl-view` couvre la session, le
  décodage et l'aperçu.

## Mise en route

```powershell
git clone https://github.com/AllanLny/webcamlink.git
cd webcamlink
cargo test --release --manifest-path pc/agent/Cargo.toml
cmake -S pc/vcam -B pc/vcam/build -A x64
cmake --build pc/vcam/build --config Release
```

La caméra virtuelle doit être enregistrée une fois en administrateur
(`pc\vcam\build\bin\Release\wcl-vcam-register.exe`) pour un test complet ; sans cela, la DLL et l'agent
restent testables seuls (`cargo test`, journal `%ProgramData%\WebcamLink\vcam.log`).

## Ce qu'on attend d'une PR

- **Petite et ciblée** : un seul sujet par PR (un correctif, une fonctionnalité).
- **Testée** : `cargo test --release` passe et la DLL compile (`cmake --build pc/vcam/build --config
  Release`). Si le changement touche la caméra virtuelle ou l'agent, décris le **test manuel** effectué
  (application cliente, format, version de Windows et d'iOS).
- **Documentée** : mets à jour le README ou `docs/` dès que le comportement visible change.
- **Sans secret** : pas de clé, d'identifiant, de jeton ni de chemin personnel.

## Le protocole WCLN est un contrat

[docs/protocol.md](docs/protocol.md) fait foi **côté iOS et côté PC**. Tout changement de message doit
être fait des deux côtés (Swift + Rust), documenté, et livré dans le même PR. La mémoire partagée
agent ↔ DLL est décrite dans [docs/shm.md](docs/shm.md) : les deux implémentations
(`pc/agent/src/shm.rs` et `pc/vcam/src/shm_reader.cpp`) doivent rester alignées.

## Style

- **Rust** : `cargo fmt` (et `cargo clippy` si possible). Pas de nouvelle dépendance sans raison.
- **C++** : C++20, WRL, mêmes conventions que `pc/vcam/src` (pas de commentaire inutile, noms explicites).
- **Langues** : code et commentaires en anglais, documentation et messages de log d'interface en
  français.
- **Commits** : à l'impératif, en anglais (c'est la langue de l'historique), un commit = un sujet.

## Signaler un bug, proposer une idée

Utilise les modèles d'issue (bug ou fonctionnalité). Pour un bug de caméra, joins :

- les dernières lignes de `%ProgramData%\WebcamLink\vcam.log` (horodatage + PID par ligne),
- la version de Windows, le modèle d'iPhone et sa version d'iOS,
- l'application cliente (app Caméra, Teams, OBS…) et le format choisi dans les réglages.

## Sécurité

Ne publie pas de vulnérabilité dans une issue publique : voir [SECURITY.md](SECURITY.md).
