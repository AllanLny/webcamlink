// WebcamLink virtual camera: an IMFMediaSourceEx that pulls NV12 frames from
// the agent through shared memory (docs/shm.md).
#include <windows.h>

#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <wrl/client.h>
#include <wrl/implements.h>

#include <atomic>
#include <mutex>
#include <vector>

#include "../include/wcl_shm.h"
#include "shm_reader.h"
#include "virtual_camera.h"

using Microsoft::WRL::ComPtr;
using Microsoft::WRL::RuntimeClass;
using Microsoft::WRL::RuntimeClassFlags;
using Microsoft::WRL::ClassicCom;
using Microsoft::WRL::InhibitFtmBase;

namespace {

constexpr UINT32 kWidth = 1920;
constexpr UINT32 kHeight = 1080;
constexpr UINT32 kFps = 30;
constexpr UINT32 kFrameBytes = kWidth * kHeight * 3 / 2;
constexpr LONGLONG kFrameDuration = 10'000'000LL / kFps;

// KSCATEGORY_VIDEO_CAMERA
const GUID kVideoCameraCategory = {
    0xe5323777, 0xf976, 0x4f5b, {0x9b, 0x55, 0xb9, 0x46, 0x99, 0xc4, 0x6e, 0x44}};

class VirtualCameraSource;

/// The single video stream exposed by the virtual camera.
class CameraStream : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IMFMediaStream2> {
public:
    CameraStream(VirtualCameraSource* source, IMFStreamDescriptor* descriptor)
        : source_(source), descriptor_(descriptor) {
        MFCreateEventQueue(&events_);
        MFCreateAttributes(&stream_attributes_, 2);
    }

    // IMFMediaEventGenerator
    STDMETHODIMP BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) override {
        return events_ ? events_->BeginGetEvent(callback, state) : E_POINTER;
    }
    STDMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) override {
        return events_ ? events_->EndGetEvent(result, event) : E_POINTER;
    }
    STDMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** event) override {
        return events_ ? events_->GetEvent(flags, event) : E_POINTER;
    }
    STDMETHODIMP QueueEvent(MediaEventType type, REFGUID extended, HRESULT status,
                            const PROPVARIANT* value) override {
        return events_ ? events_->QueueEventParamVar(type, extended, status, value) : E_POINTER;
    }

    // IMFMediaStream
    STDMETHODIMP GetMediaSource(IMFMediaSource** source) override;
    STDMETHODIMP GetStreamDescriptor(IMFStreamDescriptor** descriptor) override {
        if (!descriptor) {
            return E_POINTER;
        }
        descriptor_.CopyTo(descriptor);
        return S_OK;
    }
    STDMETHODIMP RequestSample(IUnknown* token) override;

    // IMFMediaStream2
    STDMETHODIMP SetStreamState(MF_STREAM_STATE state) override {
        std::lock_guard<std::mutex> lock(mutex_);
        state_ = state;
        return S_OK;
    }
    STDMETHODIMP GetStreamState(MF_STREAM_STATE* state) override {
        if (!state) {
            return E_POINTER;
        }
        std::lock_guard<std::mutex> lock(mutex_);
        *state = state_;
        return S_OK;
    }

    /// State update driven by the media source (it owns the transition).
    void SetStreamStateStateOnly(MF_STREAM_STATE state) {
        std::lock_guard<std::mutex> lock(mutex_);
        state_ = state;
    }

    void Shutdown() {
        if (events_) {
            events_->Shutdown();
        }
    }

    /// Frames delivered per second, filled by the pipeline for the source.
    IMFAttributes* StreamAttributes() { return stream_attributes_.Get(); }

    void SetShm(wcl::ShmReader* shm) { shm_ = shm; }

private:
    HRESULT DeliverSample(IUnknown* token);
    void FillPlaceholder(std::vector<uint8_t>& buffer) const;

    VirtualCameraSource* source_ = nullptr;
    ComPtr<IMFStreamDescriptor> descriptor_;
    ComPtr<IMFMediaEventQueue> events_;
    ComPtr<IMFAttributes> stream_attributes_;
    wcl::ShmReader* shm_ = nullptr;
    std::mutex mutex_;
    MF_STREAM_STATE state_ = MF_STREAM_STATE_STOPPED;
    LONGLONG time_ = 0;
    UINT32 delivered_ = 0;
    std::vector<uint8_t> frame_;
};

/// The virtual camera media source.
class VirtualCameraSource
    : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IMFMediaSourceEx,
                          IMFSampleAllocatorControl, IMFGetService> {
public:
    friend class CameraStream;

    VirtualCameraSource() = default;

    /// Explicit QueryInterface: it hides WRL's implementation so every request
    /// is logged — that log is how the missing interface gets identified.
    STDMETHODIMP QueryInterface(REFIID riid, void** object) {
        if (!object) {
            return E_POINTER;
        }
        *object = nullptr;
        wchar_t guid[64]{};
        StringFromGUID2(riid, guid, 64);
        wchar_t line[192];

        void* found = nullptr;
        if (IsEqualIID(riid, IID_IUnknown) ||
            IsEqualIID(riid, __uuidof(IMFMediaSource)) ||
            IsEqualIID(riid, __uuidof(IMFMediaEventGenerator))) {
            found = static_cast<IMFMediaSourceEx*>(this);
        } else if (IsEqualIID(riid, __uuidof(IMFMediaSourceEx))) {
            found = static_cast<IMFMediaSourceEx*>(this);
        } else if (IsEqualIID(riid, __uuidof(IMFSampleAllocatorControl))) {
            found = static_cast<IMFSampleAllocatorControl*>(this);
        } else if (IsEqualIID(riid, __uuidof(IMFGetService))) {
            found = static_cast<IMFGetService*>(this);
        }

        if (!found) {
            swprintf_s(line, L"source::QueryInterface(%s) -> E_NOINTERFACE", guid);
            wcl::LogLine(line);
            return E_NOINTERFACE;
        }
        *object = found;
        AddRef();
        swprintf_s(line, L"source::QueryInterface(%s) -> OK", guid);
        wcl::LogLine(line);
        return S_OK;
    }

    HRESULT Initialize() {
        // Source attributes: the Frame Server reads these (MF_E_ATTRIBUTENOTFOUND
        // otherwise) to recognise a video capture device source.
        HRESULT hr = MFCreateAttributes(&source_attributes_, 6);
        if (FAILED(hr)) {
            return hr;
        }
        source_attributes_->SetString(MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, L"WebcamLink");
        source_attributes_->SetUINT32(MF_SA_D3D_AWARE, 0);

        // One NV12 video stream, 1920x1080 @ 30 fps.
        ComPtr<IMFMediaType> type;
        hr = MFCreateMediaType(&type);
        if (FAILED(hr)) {
            return hr;
        }
        type->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video);
        type->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_NV12);
        MFSetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, kWidth, kHeight);
        MFSetAttributeRatio(type.Get(), MF_MT_FRAME_RATE, kFps, 1);
        MFSetAttributeRatio(type.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1);
        type->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive);
        // The Frame Server reads these; a missing one surfaces as
        // MF_E_ATTRIBUTENOTFOUND while it sets up the stream.
        type->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE);
        type->SetUINT32(MF_MT_FIXED_SIZE_SAMPLES, TRUE);
        type->SetUINT32(MF_MT_COMPRESSED, FALSE);
        type->SetUINT32(MF_MT_SAMPLE_SIZE, kFrameBytes);
        type->SetUINT32(MF_MT_DEFAULT_STRIDE, kWidth);
        type->SetUINT32(MF_MT_AVG_BITRATE, 20'000'000);

        IMFMediaType* types[] = {type.Get()};
        hr = MFCreateStreamDescriptor(0, 1, types, &stream_descriptor_);
        if (FAILED(hr)) {
            return hr;
        }

        camera_stream_ = Microsoft::WRL::Make<CameraStream>(this, stream_descriptor_.Get());
        if (!camera_stream_) {
            return E_OUTOFMEMORY;
        }
        camera_stream_->SetShm(&shm_);

        hr = MFCreateEventQueue(&events_);
        if (FAILED(hr)) {
            return hr;
        }
        IMFStreamDescriptor* streams[] = {stream_descriptor_.Get()};
        hr = MFCreatePresentationDescriptor(1, streams, &presentation_descriptor_);
        if (FAILED(hr)) {
            return hr;
        }
        return presentation_descriptor_->SelectStream(0);
    }

    // IMFMediaEventGenerator
    STDMETHODIMP BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) override {
        return events_ ? events_->BeginGetEvent(callback, state) : E_POINTER;
    }
    STDMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) override {
        return events_ ? events_->EndGetEvent(result, event) : E_POINTER;
    }
    STDMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** event) override {
        return events_ ? events_->GetEvent(flags, event) : E_POINTER;
    }
    STDMETHODIMP QueueEvent(MediaEventType type, REFGUID extended, HRESULT status,
                            const PROPVARIANT* value) override {
        return events_ ? events_->QueueEventParamVar(type, extended, status, value) : E_POINTER;
    }

    // IMFMediaSource
    STDMETHODIMP GetCharacteristics(DWORD* characteristics) override {
        if (!characteristics) {
            return E_POINTER;
        }
        *characteristics = MFMEDIASOURCE_IS_LIVE | MFMEDIASOURCE_CAN_PAUSE;
        return S_OK;
    }
    STDMETHODIMP CreatePresentationDescriptor(IMFPresentationDescriptor** descriptor) override {
        if (!descriptor) {
            return E_POINTER;
        }
        return presentation_descriptor_.CopyTo(descriptor);
    }
    STDMETHODIMP Start(IMFPresentationDescriptor* descriptor, const GUID* format,
                       const PROPVARIANT* position) override {
        if (state_ == MF_STREAM_STATE_RUNNING) {
            return MF_E_INVALID_STATE_TRANSITION;
        }
        if (!descriptor) {
            return E_INVALIDARG;
        }
        BOOL selected = FALSE;
        IMFStreamDescriptor* stream = nullptr;
        HRESULT hr = descriptor->GetStreamDescriptorByIndex(0, &selected, &stream);
        if (FAILED(hr)) {
            return hr;
        }
        stream->Release();
        if (!selected) {
            return MF_E_INVALIDREQUEST;
        }

        // Announce the request: the agent polls this and starts the iPhone.
        if (!shm_.Open(kWidth, kHeight, kFps)) {
            return MF_E_SHUTDOWN;
        }
        state_ = MF_STREAM_STATE_RUNNING;
        camera_stream_->SetStreamStateStateOnly(MF_STREAM_STATE_RUNNING);
        hr = events_->QueueEventParamUnk(MESourceStarted, GUID_NULL, S_OK, nullptr);
        if (SUCCEEDED(hr)) {
            hr = camera_stream_->QueueEvent(MEStreamStarted, GUID_NULL, S_OK, nullptr);
        }
        return hr;
    }
    STDMETHODIMP Stop() override {
        if (state_ == MF_STREAM_STATE_STOPPED) {
            return S_OK;
        }
        state_ = MF_STREAM_STATE_STOPPED;
        shm_.Close();
        camera_stream_->SetStreamStateStateOnly(MF_STREAM_STATE_STOPPED);
        HRESULT hr = events_->QueueEventParamUnk(MESourceStopped, GUID_NULL, S_OK, nullptr);
        if (SUCCEEDED(hr)) {
            hr = camera_stream_->QueueEvent(MEStreamStopped, GUID_NULL, S_OK, nullptr);
        }
        return hr;
    }
    STDMETHODIMP Pause() override {
        if (state_ != MF_STREAM_STATE_RUNNING) {
            return MF_E_INVALID_STATE_TRANSITION;
        }
        state_ = MF_STREAM_STATE_PAUSED;
        return events_->QueueEventParamUnk(MESourcePaused, GUID_NULL, S_OK, nullptr);
    }
    STDMETHODIMP Shutdown() override {
        if (events_) {
            events_->Shutdown();
        }
        camera_stream_->Shutdown();
        shm_.Close();
        state_ = MF_STREAM_STATE_STOPPED;
        return S_OK;
    }

    // IMFMediaSourceEx
    STDMETHODIMP GetSourceAttributes(IMFAttributes** attributes) override {
        if (!attributes) {
            return E_POINTER;
        }
        return source_attributes_.CopyTo(attributes);
    }
    STDMETHODIMP GetStreamAttributes(DWORD stream_id, IMFAttributes** attributes) override {
        if (!attributes) {
            return E_POINTER;
        }
        if (stream_id != 0) {
            return MF_E_INVALIDSTREAMNUMBER;
        }
        return camera_stream_->StreamAttributes()->QueryInterface(IID_PPV_ARGS(attributes));
    }
    STDMETHODIMP SetD3DManager(IUnknown* manager) override { return S_OK; }

    // IMFSampleAllocatorControl — required by the Frame Server for virtual
    // cameras. We always deliver our own samples built from shared memory.
    STDMETHODIMP SetDefaultAllocator(DWORD stream_id, IUnknown* allocator) override {
        if (stream_id != 0) {
            return MF_E_INVALIDSTREAMNUMBER;
        }
        return S_OK;
    }
    STDMETHODIMP GetAllocatorUsage(DWORD stream_id, DWORD* input_id,
                                   MFSampleAllocatorUsage* usage) override {
        if (stream_id != 0) {
            return MF_E_INVALIDSTREAMNUMBER;
        }
        if (input_id) {
            *input_id = 0;
        }
        if (usage) {
            *usage = MFSampleAllocatorUsage_UsesCustomAllocator;
        }
        return S_OK;
    }

    // IMFGetService — the Frame Server probes services; answer MF_E_UNSUPPORTED_SERVICE
    // rather than refusing the interface.
    STDMETHODIMP GetService(REFGUID service, REFIID riid, LPVOID* object) override {
        if (!object) {
            return E_POINTER;
        }
        *object = nullptr;
        return MF_E_UNSUPPORTED_SERVICE;
    }

    wcl::ShmReader* Shm() { return &shm_; }
    MF_STREAM_STATE State() { return state_; }

private:
    ComPtr<IMFStreamDescriptor> stream_descriptor_;
    ComPtr<IMFPresentationDescriptor> presentation_descriptor_;
    ComPtr<IMFMediaEventQueue> events_;
    ComPtr<IMFAttributes> source_attributes_;
    ComPtr<CameraStream> camera_stream_;
    wcl::ShmReader shm_;
    MF_STREAM_STATE state_ = MF_STREAM_STATE_STOPPED;
};

HRESULT CameraStream::GetMediaSource(IMFMediaSource** source) {
    if (!source) {
        return E_POINTER;
    }
    *source = nullptr;
    return source_->QueryInterface(IID_PPV_ARGS(source));
}

HRESULT CameraStream::RequestSample(IUnknown* token) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (state_ != MF_STREAM_STATE_RUNNING) {
        return MF_E_MEDIA_SOURCE_NOT_STARTED;
    }
    return DeliverSample(token);
}

HRESULT CameraStream::DeliverSample(IUnknown* token) {
    if (source_->State() != MF_STREAM_STATE_RUNNING) {
        return MF_E_MEDIA_SOURCE_NOT_STARTED;
    }

    // Wait for a frame from the agent, but keep the pipeline alive with a
    // placeholder when nothing arrives (camera open, iPhone not streaming).
    UINT32 width = 0;
    UINT32 height = 0;
    const bool has_frame = shm_ && shm_->ReadFrame(frame_, width, height, kFps > 0 ? 100 : 100);
    if (!has_frame || width != kWidth || height != kHeight ||
        frame_.size() != static_cast<size_t>(kFrameBytes)) {
        FillPlaceholder(frame_);
    }

    ComPtr<IMFMediaBuffer> buffer;
    HRESULT hr = MFCreateMemoryBuffer(kFrameBytes, &buffer);
    if (FAILED(hr)) {
        return hr;
    }
    BYTE* data = nullptr;
    hr = buffer->Lock(&data, nullptr, nullptr);
    if (FAILED(hr)) {
        return hr;
    }
    memcpy(data, frame_.data(), kFrameBytes);
    buffer->Unlock();
    buffer->SetCurrentLength(kFrameBytes);

    ComPtr<IMFSample> sample;
    hr = MFCreateSample(&sample);
    if (SUCCEEDED(hr)) {
        hr = sample->AddBuffer(buffer.Get());
    }
    if (SUCCEEDED(hr)) {
        hr = sample->SetSampleTime(time_);
    }
    if (SUCCEEDED(hr)) {
        hr = sample->SetSampleDuration(kFrameDuration);
    }
    if (FAILED(hr)) {
        return hr;
    }
    if (token) {
        sample->SetUnknown(MFSampleExtension_Token, token);
    }
    time_ += kFrameDuration;
    delivered_++;

    return events_->QueueEventParamUnk(MEMediaSample, GUID_NULL, S_OK, sample.Get());
}

void CameraStream::FillPlaceholder(std::vector<uint8_t>& buffer) const {
    buffer.resize(kFrameBytes);
    // Dark blue-ish gradient so users see the camera is alive but idle.
    uint8_t* y = buffer.data();
    for (UINT32 row = 0; row < kHeight; ++row) {
        const uint8_t value = static_cast<uint8_t>(24 + (row * 40) / kHeight);
        memset(y + row * kWidth, value, kWidth);
    }
    memset(y + kWidth * kHeight, 110, kFrameBytes - kWidth * kHeight);
}

}  // namespace

namespace wcl {

HRESULT CreateVirtualCamera(IUnknown** source) {
    if (!source) {
        return E_POINTER;
    }
    auto camera = Microsoft::WRL::Make<VirtualCameraSource>();
    if (!camera) {
        return E_OUTOFMEMORY;
    }
    HRESULT hr = camera->Initialize();
    if (FAILED(hr)) {
        return hr;
    }
    return camera.CopyTo(source);
}

}  // namespace wcl
