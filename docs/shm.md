# WebcamLink — mémoire partagée agent ↔ webcam virtuelle

Source unique de vérité pour l'échange d'images entre l'agent Rust (session utilisateur) et la DLL
de webcam virtuelle (hébergée par le **Frame Server**, session 0, compte `LOCAL SERVICE`).

Deux objets noyau nommés globalement (préfixe `Global\` pour être visible entre sessions) :

| Objet                | Type     | Rôle |
|----------------------|----------|------|
| `Global\WclShm`      | mapping  | en-tête + 3 slots NV12 |
| `Global\WclFrameReady` | event  | auto-reset, signalé par le producteur à chaque frame publiée |

Contrôle (ouverture/fermeture de la caméra) : par **polling** de l'en-tête (~4 Hz) — pas d'event,
pour rester simple et robuste.

> **Qui crée quoi.** Un processus non élevé ne peut pas créer d'objet `Global\`
> (`SeCreateGlobalPrivilege` filtré). C'est donc le **consommateur (DLL, session 0, LOCAL SERVICE)**
> qui crée le mapping et l'événement, et **l'agent (session utilisateur) qui les ouvre** — ce qui
> correspond au démarrage à la demande : l'ouverture de la caméra crée les objets, l'agent les
> détecte en réessayant `OpenFileMapping` (~1 Hz), démarre le flux, et les referme quand
> `consumer_active` repasse à 0.

En test hors session 0, les mêmes objets sont créés sous `Local\` (noms paramétrables).

## Droits d'accès

La DACL du mapping et de l'événement autorise :

- `NT AUTHORITY\SYSTEM` : contrôle total ;
- `NT AUTHORITY\LOCAL SERVICE` : contrôle total (Frame Server) ;
- `BUILTIN\Users` : `GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE`.

Sans ces droits, `OpenFileMapping` échoue depuis le Frame Server (`ERROR_ACCESS_DENIED`).

## En-tête (`WclShmHeader`, 128 octets, aligné 64)

| Offset | Taille | Champ | Écrit par | Notes |
|-------:|-------:|-------|-----------|-------|
| 0  | 8  | `magic`             | producteur | ASCII `WCLSHM01` |
| 8  | 4  | `version`           | producteur | 1 |
| 12 | 4  | `header_size`       | producteur | 128 |
| 16 | 4  | `slot_count`        | producteur | 3 |
| 20 | 4  | `slot_capacity`     | producteur | octets NV12 max (4K = 12 441 600) |
| 24 | 4  | `width`             | producteur | largeur courante |
| 28 | 4  | `height`            | producteur | hauteur courante |
| 32 | 4  | `stride`            | producteur | = `width` (NV12 compacté) |
| 36 | 4  | `pixel_format`      | producteur | 0 = NV12 |
| 40 | 4  | `frame_bytes`       | producteur | `width*height*3/2` |
| 44 | 4  | `producer_state`    | producteur | 0 arrêté, 1 flux actif, 2 erreur |
| 48 | 4  | `producer_error`    | producteur | code `Error` du protocole (0 = aucun) |
| 52 | 4  | `latest_slot`       | producteur | index du dernier slot prêt, `0xFFFFFFFF` = aucun |
| 56 | 4  | `consumer_active`   | **consommateur** | 1 quand une appli a la caméra ouverte |
| 60 | 4  | `consumer_width`    | **consommateur** | largeur demandée par l'appli |
| 64 | 4  | `consumer_height`   | **consommateur** | hauteur demandée |
| 68 | 4  | `consumer_fps`      | **consommateur** | images/s demandées |
| 72 | 4  | `consumer_generation` | **consommateur** | incrémenté à chaque ouverture |
| 76 | 4  | `producer_generation` | producteur | incrémenté à chaque (re)configuration |
| 80 | 8  | `pts_us`            | producteur | horodatage de la dernière frame publiée |
| 88 | 8  | `sequence`          | producteur | compteur global de publications |
| 96 | 8  | `frames_published`  | producteur | statistiques |
| 104| 8  | `frames_dropped`    | producteur | slots écrasés avant lecture |
| 112| 16 | réservé             | | |

## Slots

Chaque slot est précédé de 64 octets d'en-tête, puis `slot_capacity` octets de payload :

| Offset | Taille | Champ | Notes |
|-------:|-------:|-------|-------|
| 0  | 4 | `state`      | 0 libre, 1 écriture en cours, 2 prêt, 3 lecture |
| 4  | 4 | `sequence`   | seqlock : impair pendant l'écriture |
| 8  | 8 | `pts_us`     | horodatage de la frame |
| 16 | 4 | `frame_bytes`| octets utiles du payload |
| 20 | 4 | `reserved`   | |
| 24 | 40| réservé      | |
| 64 | `slot_capacity` | `payload` | NV12 compacté (Y puis UV) |

Offsets absolus dans le mapping :

```
slot_base(i) = 128 + i * (64 + slot_capacity)
```

## Protocole producteur (Rust)

1. Choisit le prochain slot (`round-robin`), le marque `state=1`, `sequence += 1`.
2. Copie les octets NV12, renseigne `pts_us`/`frame_bytes`.
3. `sequence += 1`, `state=2`, met `latest_slot`, `sequence` global et `pts_us` à jour.
4. `SetEvent(WclFrameReady)` ; si le slot suivant est encore `state=2` (pas lu), incrémente
   `frames_dropped` et l'écrase (l'image la plus récente gagne).

Le producteur lit `consumer_active`/`consumer_*` à chaque publication et :
- démarre la session (`SetFormat` + `Start`) quand `consumer_active` passe à 1 ;
- envoie `Stop` après 2 s sans consommateur ;
- applique un changement de `consumer_width/height` (nouveau `SetFormat` + IDR).

## Protocole consommateur (DLL)

1. `OpenFileMapping` + `MapViewOfFile` (droits ci-dessus).
2. Écrit `consumer_width/height/fps`, puis `consumer_active=1` et `consumer_generation += 1`.
3. Attend `WclFrameReady` (timeout 100 ms), lit `latest_slot`, copie le slot (seqlock : relire
   `sequence` avant/après ; abandonner si impair ou changé).
4. Sans frame depuis > 1 s : renvoie une image de remplacement (dégradé sombre), sans erreur.
5. À la fermeture : `consumer_active=0`.

## Placeholder

La DLL génère elle-même l'image « en attente » (dégradé sombre + bandeau) — inutile de faire
transiter un placeholder par la mémoire partagée.
