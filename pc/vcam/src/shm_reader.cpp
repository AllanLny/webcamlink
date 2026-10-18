#include "shm_reader.h"

#include <aclapi.h>
#include <sddl.h>

#include <cstring>

namespace {

/// SECURITY_ATTRIBUTES allowing SYSTEM and LOCAL SERVICE (Frame Server) full
/// access plus interactive users read/write/synchronize.
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
        AddAccessAllowedAce(acl_ptr, ACL_REVISION,
                            GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE, users_sid);
        SetSecurityDescriptorDacl(&descriptor, TRUE, acl_ptr, FALSE);

        attributes.nLength = sizeof(SECURITY_ATTRIBUTES);
        attributes.lpSecurityDescriptor = &descriptor;
        attributes.bInheritHandle = FALSE;
    }
};

inline uint32_t read_u32(const uint8_t* base, uint32_t offset) {
    uint32_t value = 0;
    std::memcpy(&value, base + offset, sizeof(value));
    return value;
}

inline void write_u32(uint8_t* base, uint32_t offset, uint32_t value) {
    std::memcpy(base + offset, &value, sizeof(value));
}

inline uint64_t read_u64(const uint8_t* base, uint32_t offset) {
    uint64_t value = 0;
    std::memcpy(&value, base + offset, sizeof(value));
    return value;
}

}  // namespace

namespace wcl {

ShmReader::~ShmReader() { Close(); }

bool ShmReader::CreateObjects() {
    SharedSecurity security;
    const uint32_t capacity = kMaxSlotCapacity;
    const uint64_t size = kHeaderSize + kSlotCount * (kSlotHeaderSize + capacity);

    mapping_ = CreateFileMappingW(INVALID_HANDLE_VALUE, &security.attributes, PAGE_READWRITE,
                                  static_cast<DWORD>(size >> 32), static_cast<DWORD>(size),
                                  kWclShmName);
    if (!mapping_) {
        return false;
    }
    const bool created = GetLastError() != ERROR_ALREADY_EXISTS;

    ready_ = CreateEventW(&security.attributes, FALSE, FALSE, kWclFrameReadyName);
    if (!ready_) {
        CloseObjects();
        return false;
    }

    view_ = static_cast<uint8_t*>(MapViewOfFile(mapping_, FILE_MAP_ALL_ACCESS, 0, 0, 0));
    if (!view_) {
        CloseObjects();
        return false;
    }
    capacity_ = capacity;

    if (created) {
        std::memset(view_, 0, kHeaderSize);
        const char magic[8] = {'W', 'C', 'L', 'S', 'H', 'M', '0', '1'};
        std::memcpy(view_ + offMagic, magic, sizeof(magic));
        write_u32(view_, offVersion, kVersion);
        write_u32(view_, offHeaderSize, kHeaderSize);
        write_u32(view_, offSlotCount, kSlotCount);
        write_u32(view_, offSlotCapacity, capacity);
        write_u32(view_, offPixelFormat, 0);
        write_u32(view_, offLatestSlot, kNoSlot);
        for (uint32_t slot = 0; slot < kSlotCount; ++slot) {
            const uint32_t base = kHeaderSize + slot * (kSlotHeaderSize + capacity);
            write_u32(view_, base + slotState, kSlotFree);
            write_u32(view_, base + slotSequence, 0);
        }
    }

    return true;
}

bool ShmReader::Open(uint32_t width, uint32_t height, uint32_t fps) {
    if (!view_ && !CreateObjects()) {
        return false;
    }
    width_ = width;
    height_ = height;
    fps_ = fps;

    write_u32(view_, offConsumerWidth, width);
    write_u32(view_, offConsumerHeight, height);
    write_u32(view_, offConsumerFps, fps);
    write_u32(view_, offConsumerGeneration, read_u32(view_, offConsumerGeneration) + 1);
    write_u32(view_, offConsumerActive, 1);
    last_slot_ = kNoSlot;
    return true;
}

void ShmReader::Close() {
    if (view_) {
        write_u32(view_, offConsumerActive, 0);
    }
    CloseObjects();
}

void ShmReader::CloseObjects() {
    if (view_) {
        UnmapViewOfFile(view_);
        view_ = nullptr;
    }
    if (ready_) {
        CloseHandle(ready_);
        ready_ = nullptr;
    }
    if (mapping_) {
        CloseHandle(mapping_);
        mapping_ = nullptr;
    }
}

uint32_t ShmReader::ProducerState() const {
    return view_ ? read_u32(view_, offProducerState) : kProducerStopped;
}

bool ShmReader::ReadFrame(std::vector<uint8_t>& nv12, uint32_t& width, uint32_t& height,
                          uint32_t timeout_ms) {
    if (!view_) {
        return false;
    }
    WaitForSingleObject(ready_, timeout_ms);

    const uint32_t latest = read_u32(view_, offLatestSlot);
    if (latest == kNoSlot || latest >= kSlotCount) {
        return false;
    }

    const uint32_t base = kHeaderSize + latest * (kSlotHeaderSize + capacity_);
    if (read_u32(view_, base + slotState) != kSlotReady) {
        return false;
    }
    const uint32_t sequence_before = read_u32(view_, base + slotSequence);
    if (sequence_before % 2 != 0) {
        return false;
    }

    const uint32_t frame_bytes = read_u32(view_, base + slotFrameBytes);
    const uint32_t frame_width = read_u32(view_, offWidth);
    const uint32_t frame_height = read_u32(view_, offHeight);
    const uint32_t expected = frame_width * frame_height * 3 / 2;
    if (frame_bytes == 0 || frame_bytes > capacity_ || frame_bytes != expected) {
        return false;
    }

    nv12.resize(frame_bytes);
    std::memcpy(nv12.data(), view_ + base + kSlotHeaderSize, frame_bytes);

    const uint32_t sequence_after = read_u32(view_, base + slotSequence);
    if (sequence_after != sequence_before) {
        return false;  // overwritten while copying
    }

    width = frame_width;
    height = frame_height;
    last_slot_ = latest;
    return true;
}

}  // namespace wcl
