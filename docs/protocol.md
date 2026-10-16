# WebcamLink — protocole v1

Source unique de vérité pour le lien iPhone ↔ PC. Toute évolution incompatible incrémente `version`.

## Transport

- TCP brut tunnelisé par USB via **usbmuxd** (côté PC : `127.0.0.1:27015`, message `Connect{DeviceID,
  PortNumber}` avec **PortNumber byte-swapped** ; côté iPhone : `NWListener` sur `127.0.0.1:5321`).
- Un seul client à la fois. Une nouvelle connexion remplace l'ancienne.
- Tous les entiers sont **little-endian**. `str` = `u16 longueur` + octets UTF-8.

## En-tête de trame (12 octets)

| Offset | Taille | Champ  | Valeur |
|-------:|-------:|--------|--------|
| 0      | 4      | magic  | ASCII `WCLN` (0x57 0x43 0x4C 0x4E) |
| 4      | 1      | version| 1 |
| 5      | 1      | type   | voir table ci-dessous |
| 6      | 2      | flags  | réservé, 0 |
| 8      | 4      | length | taille du payload, max 16 MiB |

Un type inconnu est ignoré ; les octets de fin inconnus dans un message connu sont ignorés ; un header
invalide (magic/version/length) ferme la connexion.

## Messages

| Type | Nom              | Sens         | Rôle |
|-----:|------------------|--------------|------|
| 0x01 | `Hello`          | ↔            | identification, plage de versions |
| 0x02 | `Caps`           | device → PC  | objectifs + formats supportés |
| 0x10 | `SetCamera`      | PC → device  | choix de l'objectif |
| 0x11 | `SetFormat`      | PC → device  | résolution, fps, codec, bitrate |
| 0x12 | `Start`          | PC → device  | démarrage du flux |
| 0x13 | `Stop`           | PC → device  | arrêt du flux |
| 0x14 | `RequestKeyframe`| PC → device  | IDR immédiat |
| 0x15 | `SetOptions`     | PC → device  | miroir, rotation, preview assombrie |
| 0x16 | `SetImage`       | PC → device  | contrôles d'image (zoom, expo, focus, WB, torche…) |
| 0x20 | `VideoConfig`    | device → PC  | SPS/PPS (Annex-B) ; la frame suivante est un IDR |
| 0x21 | `VideoFrame`     | device → PC  | une access unit H.264 |
| 0x30 | `Heartbeat`      | ↔            | liveness (1 s) |
| 0x31 | `Status`         | device → PC  | thermique, batterie, bitrate/fps réels |
| 0x7F | `Error`          | ↔            | erreur |

### 0x01 Hello

| Champ | Type | Notes |
|-------|------|-------|
| `min_version` | u8 | plage de versions acceptée |
| `max_version` | u8 | |
| `role` | u8 | 0 = PC (host), 1 = iPhone (device) |
| `app_version` | str | ex. `0.1.0` |
| `name` | str | nom de l'appareil |
| `model` | str | ex. `iPhone16,1` / `Windows 11` |

Les deux côtés envoient `Hello` juste après la connexion.

### 0x02 Caps

| Champ | Type | Notes |
|-------|------|-------|
| `camera_count` | u8 | |
| `cameras[]` | | × `camera_count` |
| ↳ `camera_id` | u8 | stable pour la connexion |
| ↳ `position` | u8 | 0 = arrière, 1 = avant |
| ↳ `lens` | u8 | 0 = large, 1 = ultra-grand-angle, 2 = télé, 3 = avant (TrueDepth), 255 = autre |
| ↳ `label` | str | ex. `Back Ultra Wide` |
| ↳ `format_count` | u16 | |
| ↳ `formats[]` | | × `format_count` : `width` u16, `height` u16, `max_fps` u16 |
| `codecs` | u8 | bit0 = H.264, bit1 = HEVC |

Formats dédupliqués par (largeur, hauteur) avec le fps max ; uniquement des formats vidéo de capture.

### 0x10 SetCamera

`camera_id` u8.

### 0x11 SetFormat

| Champ | Type | Notes |
|-------|------|-------|
| `width` | u16 | |
| `height` | u16 | |
| `fps` | u16 | ≤ `max_fps` du format |
| `codec` | u8 | 0 = H.264, 1 = HEVC |
| `bitrate_kbps` | u32 | 0 = device choisit |

### 0x12 Start / 0x13 Stop / 0x14 RequestKeyframe

Payload vide.

### 0x15 SetOptions

| Champ | Type | Notes |
|-------|------|-------|
| `mirror` | u8 | 0/1 |
| `rotation` | u16 | 0, 90, 180, 270 |
| `dim_preview` | u8 | 0/1 — assombrit l'aperçu iPhone |

### 0x16 SetImage (v1 minimal, complété en P3)

| Champ | Type | Notes |
|-------|------|-------|
| `zoom` | u16 | ×100 (100 = 1.0×) |
| `exposure_mode` | u8 | 0 = auto, 1 = bias, 2 = manuel |
| `exposure_bias_ev` | i16 | ×100 |
| `iso` | u16 | manuel |
| `exposure_duration_us` | u32 | manuel |
| `focus_mode` | u8 | 0 = continu, 1 = verrouillé, 2 = point+verrou |
| `focus_x` / `focus_y` | u16 | ×10000, point normalisé |
| `wb_mode` | u8 | 0 = auto, 1 = verrouillé, 2 = manuel |
| `wb_temp` / `wb_tint` | u16 | Kelvin / ×100 |
| `torch` | u8 | 0 = off, 1 = on |
| `stabilization` | u8 | 0 = off, 1 = standard |
| `hdr` | u8 | 0 = off, 1 = on |

Un champ absent est laissé inchangé ; une valeur inconnue est ignorée.

### 0x20 VideoConfig

| Champ | Type | Notes |
|-------|------|-------|
| `codec` | u8 | 0 = H.264, 1 = HEVC |
| `width` / `height` | u16 | taille encodée (après rotation) |
| `fps` | u16 | nominal |
| `parameter_sets` | u32 longueur + bytes | Annex-B : SPS + PPS (+ VPS si HEVC) |

### 0x21 VideoFrame

| Champ | Type | Notes |
|-------|------|-------|
| `pts_us` | u64 | horloge monotone du device |
| `frame_flags` | u8 | bit0 = keyframe (IDR) |
| `data` | reste du payload | une access unit Annex-B |

### Contraintes du flux H.264

- **Pas de B-frames** (`AllowFrameReordering = false` côté VideoToolbox) : le décodeur PC tourne en
  mode low-latency, donc sans file de réordonnancement — l'ordre de décodage doit être l'ordre
  d'affichage.
- IDR au plus tard toutes les 2 s, et immédiatement sur `RequestKeyframe` ; le payload d'un IDR
  embarque SPS/PPS en bande (en plus du `VideoConfig` de la session).
- Le décodeur Media Foundation en mode low-latency ne garantit pas les horodatages de sortie :
  `pts_us` sert à la mesure et au diagnostic (latence, gigue), pas à réordonner les frames.

### 0x30 Heartbeat

`send_time_us` u64 (horloge de l'émetteur). Le pair répond avec son propre heartbeat. Silence total
pendant 3 s = connexion morte.

### 0x31 Status

`thermal_state` u8 (0 nominal → 3 critical), `battery_percent` u8 (255 = inconnu), `charging` u8,
`actual_bitrate_kbps` u32, `actual_fps_x100` u16, `camera_id` u8 (255 = inconnu).

### 0x7F Error

`code` u16, `message` str.

| Code | Sens |
|-----:|------|
| 1 | `UNSUPPORTED_VERSION` |
| 2 | `CAMERA_PERMISSION_DENIED` |
| 3 | `INVALID_FORMAT` |
| 4 | `CAMERA_UNAVAILABLE` (utilisée ailleurs ou app en arrière-plan) |
| 5 | `ENCODER_FAILURE` |
| 6 | `THERMAL_SHUTDOWN` |
| 7 | `NOT_IN_FOREGROUND` |

## Session type

```
PC                                    iPhone
 |---- Connect(port 5321) ------------->|
 |---- Hello -------------------------->|
 |<--------------------------- Hello ---|
 |<---------------------------- Caps ---|
 |---- SetCamera(0) ------------------->|
 |---- SetOptions(mirror,rot,dim) ----->|
 |---- SetFormat(1920,1080,30,H264,0) ->|
 |---- Start -------------------------->|
 |<--------------------- VideoConfig ---|
 |<--------------------- VideoFrame -----|
 |<--------------------- Status ---------|  (2 s)
 |---- Heartbeat ---------------------->|  (1 s)
 |---- SetImage(zoom=200) ------------->|
 |---- Stop --------------------------->|
```

## Versionnage

- Ajouter un type de message ou des champs en fin de message : pas de changement de version.
- Changer la sémantique/layout d'un champ existant : incrémenter `version` et continuer à gérer l'ancien
  au moins une release.
