#pragma once

#include <windows.h>

// {B7C0E1F2-3A4D-4E5F-9A6B-1C2D3E4F5A6B}
static const CLSID CLSID_WebcamLinkVirtualCamera = {
    0xb7c0e1f2, 0x3a4d, 0x4e5f, {0x9a, 0x6b, 0x1c, 0x2d, 0x3e, 0x4f, 0x5a, 0x6b}};

static const wchar_t* kWclShmName = L"Global\\WclShm";
static const wchar_t* kWclFrameReadyName = L"Global\\WclFrameReady";

// Shared memory layout — keep in sync with docs/shm.md and pc/agent/src/shm.rs.
namespace wcl {

constexpr uint32_t kMagic0 = 0x4C4357;  // "WCL"
constexpr uint32_t kVersion = 1;
constexpr uint32_t kHeaderSize = 128;
constexpr uint32_t kSlotHeaderSize = 64;
constexpr uint32_t kSlotCount = 3;
constexpr uint32_t kMaxSlotCapacity = 3840u * 2160u * 3u / 2u;
constexpr uint32_t kNoSlot = 0xFFFFFFFFu;

constexpr uint32_t kSlotFree = 0;
constexpr uint32_t kSlotWriting = 1;
constexpr uint32_t kSlotReady = 2;

constexpr uint32_t kProducerStopped = 0;
constexpr uint32_t kProducerStreaming = 1;
constexpr uint32_t kProducerError = 2;

// Header offsets.
constexpr uint32_t offMagic = 0;
constexpr uint32_t offVersion = 8;
constexpr uint32_t offHeaderSize = 12;
constexpr uint32_t offSlotCount = 16;
constexpr uint32_t offSlotCapacity = 20;
constexpr uint32_t offWidth = 24;
constexpr uint32_t offHeight = 28;
constexpr uint32_t offStride = 32;
constexpr uint32_t offPixelFormat = 36;
constexpr uint32_t offFrameBytes = 40;
constexpr uint32_t offProducerState = 44;
constexpr uint32_t offProducerError = 48;
constexpr uint32_t offLatestSlot = 52;
constexpr uint32_t offConsumerActive = 56;
constexpr uint32_t offConsumerWidth = 60;
constexpr uint32_t offConsumerHeight = 64;
constexpr uint32_t offConsumerFps = 68;
constexpr uint32_t offConsumerGeneration = 72;
constexpr uint32_t offProducerGeneration = 76;
constexpr uint32_t offPtsUs = 80;
constexpr uint32_t offSequence = 88;
constexpr uint32_t offFramesPublished = 96;
constexpr uint32_t offFramesDropped = 104;

// Slot offsets.
constexpr uint32_t slotState = 0;
constexpr uint32_t slotSequence = 4;
constexpr uint32_t slotPtsUs = 8;
constexpr uint32_t slotFrameBytes = 16;

}  // namespace wcl
