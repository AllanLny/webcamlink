#include "wcl_controls.h"

#include <aclapi.h>

#include <cstring>

namespace {

/// Mêmes droits que la mémoire partagée des images : SYSTEM et LOCAL SERVICE
/// (Frame Server) en contrôle total, les utilisateurs en lecture/écriture.
struct SharedSecurity {
    SECURITY_DESCRIPTOR descriptor{};
    ACL acl{};
    BYTE acl_buffer[512]{};
    BYTE system_sid[SECURITY_MAX_SID_SIZE]{};
    BYTE service_sid[SECURITY_MAX_SID_SIZE]{};
    BYTE users_sid[SECURITY_MAX_SID_SIZE]{};
    SECURITY_ATTRIBUTES attributes{};

    SharedSecurity() {
        InitializeSecurityDescriptor(&descriptor, SECURITY_DESCRIPTOR_REVISION);
        InitializeAcl(reinterpret_cast<ACL*>(acl_buffer), sizeof(acl_buffer), ACL_REVISION);

        DWORD size = sizeof(system_sid);
        CreateWellKnownSid(WinLocalSystemSid, nullptr, system_sid, &size);
        size = sizeof(service_sid);
        CreateWellKnownSid(WinLocalServiceSid, nullptr, service_sid, &size);
        size = sizeof(users_sid);
        CreateWellKnownSid(WinBuiltinUsersSid, nullptr, users_sid, &size);

        ACL* acl_ptr = reinterpret_cast<ACL*>(acl_buffer);
        AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_ALL, system_sid);
        AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_ALL, service_sid);
        AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_READ | GENERIC_WRITE, users_sid);
        SetSecurityDescriptorDacl(&descriptor, TRUE, acl_ptr, FALSE);

        attributes.nLength = sizeof(SECURITY_ATTRIBUTES);
        attributes.lpSecurityDescriptor = &descriptor;
        attributes.bInheritHandle = FALSE;
    }
};

}  // namespace

namespace wcl {

ControlsMapping::~ControlsMapping() { Close(); }

void ControlsMapping::Close() {
    if (block_) {
        UnmapViewOfFile(block_);
        block_ = nullptr;
    }
    if (mapping_) {
        CloseHandle(mapping_);
        mapping_ = nullptr;
    }
}

bool ControlsMapping::Create() {
    if (block_) {
        return true;
    }

    SharedSecurity security;
    mapping_ = CreateFileMappingW(INVALID_HANDLE_VALUE, &security.attributes, PAGE_READWRITE, 0,
                                  sizeof(ControlsBlock), L"Global\\WclControls");
    if (!mapping_) {
        return false;
    }
    const bool created = GetLastError() != ERROR_ALREADY_EXISTS;

    block_ = static_cast<ControlsBlock*>(MapViewOfFile(mapping_, FILE_MAP_ALL_ACCESS, 0, 0, 0));
    if (!block_) {
        Close();
        return false;
    }

    if (created) {
        std::memset(block_, 0, sizeof(ControlsBlock));
        block_->version = kControlsVersion;
        block_->size = sizeof(ControlsBlock);
    }
    return true;
}

void ControlsMapping::BumpClientGeneration() {
    if (!block_) {
        return;
    }
    // Un seul écrivain à la fois côté clients (l'interface est appelée depuis le
    // pipeline), la génération sert de compteur de révision.
    ++block_->client_generation;
}

}  // namespace wcl
