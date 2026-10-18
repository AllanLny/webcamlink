#pragma once

#include <cstdint>
#include <vector>

#include "wcl_shm.h"

namespace wcl {

/// Consumer side of the shared memory: creates the objects (or opens them when
/// they already exist), publishes the consumer request, and copies frames out.
class ShmReader {
public:
    ShmReader() = default;
    ~ShmReader();

    ShmReader(const ShmReader&) = delete;
    ShmReader& operator=(const ShmReader&) = delete;

    /// Create/open the objects and announce the requested format.
    bool Open(uint32_t width, uint32_t height, uint32_t fps);

    /// Announce that nobody uses the camera anymore.
    void Close();

    /// Wait for a new frame (auto-reset event) and copy the latest slot.
    /// Returns false when no frame could be read (timeout / producer idle).
    bool ReadFrame(std::vector<uint8_t>& nv12, uint32_t& width, uint32_t& height,
                   uint32_t timeout_ms);

    bool IsOpen() const { return view_ != nullptr; }
    uint32_t ProducerState() const;

private:
    bool CreateObjects();
    void CloseObjects();

    HANDLE mapping_ = nullptr;
    HANDLE ready_{nullptr};
    uint8_t* view_ = nullptr;
    uint32_t capacity_ = 0;
    uint32_t width_ = 0;
    uint32_t height_ = 0;
    uint32_t fps_ = 0;
    uint32_t last_slot_ = wcl::kNoSlot;
};

}  // namespace wcl
