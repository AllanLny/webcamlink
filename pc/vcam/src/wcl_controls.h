#pragma once

// Réglages d'image exposés aux applications clientes (OBS, boîte de dialogue
// caméra de Windows) : un mapping `Global\WclControls` distinct de
// `Global\WclShm`. Une DLL plus ancienne ne le crée pas — la vidéo continue de
// fonctionner et seule la partie « réglages pilotés par les clients » reste
// inactive, ce que l'agent détecte à l'ouverture.

#include <windows.h>

#include <cstdint>

namespace wcl {

// --- Propriétés KS (les identifiants et propriétés de ksmedia.h sont répétés
// ici : inclure ks.h casse les en-têtes Media Foundation via cguid.h) ---

constexpr GUID kPropsetVideoProcAmp = {
    0xC6E13360, 0x30AC, 0x11D0, {0xA1, 0x8C, 0x00, 0xA0, 0xC9, 0x11, 0x89, 0x56}};
constexpr GUID kPropsetCameraControl = {
    0xC6E13370, 0x30AC, 0x11D0, {0xA1, 0x8C, 0x00, 0xA0, 0xC9, 0x11, 0x89, 0x56}};

/// Seules les propriétés que l'iPhone sait honorer.
constexpr ULONG kVideoProcAmpWhiteBalance = 7;
constexpr ULONG kVideoProcAmpGain = 9;
constexpr ULONG kCameraControlZoom = 3;
constexpr ULONG kCameraControlExposure = 4;
constexpr ULONG kCameraControlFocus = 6;

/// Drapeaux auto/manuel des valeurs transmises par les clients.
constexpr ULONG kCameraControlFlagAuto = 0x0001;
constexpr ULONG kCameraControlFlagManual = 0x0002;

/// Types de requête (ks.h).
constexpr ULONG kPropertyTypeGet = 0x00000001;
constexpr ULONG kPropertyTypeSet = 0x00000002;
constexpr ULONG kPropertyTypeBasicSupport = 0x20000000;

struct KsPropertyRequest {
    GUID set;
    ULONG id;
    ULONG flags;
};

/// `KSPROPERTY_VIDEOPROCAMP_S` et `KSPROPERTY_CAMERACONTROL_S` ont la même
/// forme. Certains clients ajoutent un champ réservé : on lit et on écrit
/// toujours les 36 premiers octets, jamais plus.
struct KsPropertyValue {
    KsPropertyRequest property;
    LONG value;
    ULONG flags;
    ULONG capabilities;
};

constexpr ULONG kKsPropertyValueSize = sizeof(KsPropertyValue);

struct KsPropertyDescription {
    ULONG access_flags;
    ULONG description_size;
    GUID property_type_set;
    ULONG members_list_count;
    ULONG reserved;
};

// --- Bloc partagé (`Global\WclControls`) ---

constexpr uint32_t kControlsVersion = 1;
constexpr uint32_t kControlsFlagAutoExposure = 1u << 0;
constexpr uint32_t kControlsFlagAutoWhiteBalance = 1u << 1;
constexpr uint32_t kControlsFlagAutoFocus = 1u << 2;

/// Structure à plat, uniquement des u32/i32 : même disposition des deux côtés
/// (`ControlsBlock` en Rust), aucun remplissage possible.
struct ControlsBlock {
    uint32_t version;
    uint32_t size;
    /// Incrémenté par la DLL à chaque demande d'un client.
    uint32_t client_generation;
    /// Dernière demande reprise par l'agent.
    uint32_t applied_generation;
    uint32_t flags;
    uint32_t zoom_x100;
    int32_t exposure_bias_ev_x100;
    uint32_t iso;
    uint32_t exposure_us;
    uint32_t white_balance_k;
    uint32_t focus_x100;

    /// Plages annoncées par la caméra (l'agent les publie pour que les clients
    /// voient des bornes justes).
    uint32_t range_valid;
    uint32_t zoom_max_x100;
    uint32_t iso_min;
    uint32_t iso_max;
    uint32_t exposure_min_us;
    uint32_t exposure_max_us;
    uint32_t wb_min_k;
    uint32_t wb_max_k;

    /// Ce qui a réellement été appliqué, relu par les GET des clients.
    uint32_t applied_flags;
    uint32_t applied_zoom_x100;
    int32_t applied_exposure_bias_ev_x100;
    uint32_t applied_iso;
    uint32_t applied_exposure_us;
    uint32_t applied_white_balance_k;
    uint32_t applied_focus_x100;
    uint32_t applied_available;
};

/// Crée (ou ouvre) le mapping de contrôle. Créé par la DLL — le Frame Server
/// tourne en session 0, seul endroit d'où un objet `Global\` est créable —,
/// ouvert par l'agent.
class ControlsMapping {
public:
    ControlsMapping() = default;
    ~ControlsMapping();

    ControlsMapping(const ControlsMapping&) = delete;
    ControlsMapping& operator=(const ControlsMapping&) = delete;

    bool Create();

    bool IsOpen() const { return block_ != nullptr; }
    ControlsBlock* Block() const { return block_; }

    /// Annonce qu'une application a demandé une valeur : l'agent la reprendra.
    void BumpClientGeneration();

private:
    void Close();

    HANDLE mapping_ = nullptr;
    ControlsBlock* block_ = nullptr;
};

}  // namespace wcl
