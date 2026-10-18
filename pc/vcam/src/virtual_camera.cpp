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
#include "config.h"
#include "logging_attributes.h"
#include "shm_reader.h"
#include "virtual_camera.h"

using Microsoft::WRL::ComPtr;
using Microsoft::WRL::RuntimeClass;
using Microsoft::WRL::RuntimeClassFlags;
using Microsoft::WRL::ClassicCom;
using Microsoft::WRL::InhibitFtmBase;

namespace {

constexpr UINT32 kAllocatedSamples = 10;

// KSCATEGORY_VIDEO_CAMERA
const GUID kVideoCameraCategory = {
    0xe5323777, 0xf976, 0x4f5b, {0x9b, 0x55, 0xb9, 0x46, 0x99, 0xc4, 0x6e, 0x44}};

/// Answers a KS operation the way a miniport driver without the property does.
HRESULT KsNotSupported(const wchar_t* method) {
    wchar_t line[128];
    swprintf_s(line, L"source::%s -> ERROR_SET_NOT_FOUND", method);
    wcl::LogLine(line);
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

/// Stream state, spelled out for the log lines.
const wchar_t* StateName(MF_STREAM_STATE state) {
    switch (state) {
        case MF_STREAM_STATE_RUNNING:
            return L"running";
        case MF_STREAM_STATE_PAUSED:
            return L"paused";
        default:
            return L"stopped";
    }
}

class VirtualCameraSource;

/// The single video stream exposed by the virtual camera.
class CameraStream : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IMFMediaStream2> {
public:
    CameraStream(VirtualCameraSource* source, IMFStreamDescriptor* descriptor,
                 IMFMediaType* media_type, const wcl::VideoConfig& config)
        : source_(source),
          descriptor_(descriptor),
          media_type_(media_type),
          width_(config.width),
          height_(config.height),
          frame_bytes_(config.width * config.height * 3 / 2),
          frame_duration_(10'000'000LL / (config.fps > 0 ? config.fps : 30)) {
        MFCreateEventQueue(&events_);
        // The Frame Server reads the stream category, identifier and frame
        // source types on both the stream and its descriptor; a missing one
        // surfaces as MF_E_ATTRIBUTENOTFOUND while it sets the stream up.
        stream_attributes_ = Microsoft::WRL::Make<wcl::LoggingAttributes>(L"stream-store");
        IMFAttributes* stores[] = {stream_attributes_ ? stream_attributes_->Store() : nullptr,
                                   descriptor_.Get()};
        for (IMFAttributes* store : stores) {
            if (!store) {
                continue;
            }
            store->SetGUID(MF_DEVICESTREAM_STREAM_CATEGORY, wcl::kPinNameVideoCapture);
            store->SetUINT32(MF_DEVICESTREAM_STREAM_ID, 0);
            store->SetUINT32(MF_DEVICESTREAM_FRAMESERVER_SHARED, 1);
            store->SetUINT32(MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
                             MFFrameSourceTypes_Color);
        }
    }

    /// Explicit QueryInterface: it answers for the base interfaces of
    /// IMFMediaStream2 (clients ask for IMFMediaStream and
    /// IMFMediaEventGenerator) and logs every request, like the source does.
    STDMETHODIMP QueryInterface(REFIID riid, void** object) override {
        if (!object) {
            return E_POINTER;
        }
        *object = nullptr;
        wchar_t guid[64]{};
        StringFromGUID2(riid, guid, 64);
        wchar_t line[192];

        if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, __uuidof(IMFMediaStream2)) ||
            IsEqualIID(riid, __uuidof(IMFMediaStream)) ||
            IsEqualIID(riid, __uuidof(IMFMediaEventGenerator))) {
            *object = static_cast<IMFMediaStream2*>(this);
            AddRef();
            swprintf_s(line, L"stream::QueryInterface(%s) -> OK", guid);
            wcl::LogLine(line);
            return S_OK;
        }
        swprintf_s(line, L"stream::QueryInterface(%s) -> E_NOINTERFACE", guid);
        wcl::LogLine(line);
        return E_NOINTERFACE;
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
        {
            std::lock_guard<std::mutex> lock(mutex_);
            state_ = state;
            if (state == MF_STREAM_STATE_RUNNING) {
                PrepareAllocator();
            }
        }
        wchar_t line[128];
        swprintf_s(line, L"stream::SetStreamState(%s) -> S_OK", StateName(state));
        wcl::LogLine(line);
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

    /// State update driven by the media source (it owns the transition). The
    /// pipeline's allocator is prepared here too: the Frame Server hands one
    /// over and never calls SetStreamState itself.
    void SetStreamStateStateOnly(MF_STREAM_STATE state) {
        std::lock_guard<std::mutex> lock(mutex_);
        state_ = state;
        if (state == MF_STREAM_STATE_RUNNING) {
            PrepareAllocator();
        }
    }

    void Shutdown() {
        if (events_) {
            events_->Shutdown();
        }
    }

    /// The stream's attribute store, handed out through GetStreamAttributes.
    IMFAttributes* StreamAttributes() { return stream_attributes_.Get(); }

    /// The pipeline's allocator: it owns the buffers the Frame Server shares
    /// between clients, so samples come from there once it is handed over. When
    /// it refuses our media type, our own buffers stay in use.
    void SetSampleAllocator(IMFVideoSampleAllocator* allocator) {
        std::lock_guard<std::mutex> lock(mutex_);
        allocator_ = allocator;
        if (state_ == MF_STREAM_STATE_RUNNING) {
            PrepareAllocator();
        }
    }

    void SetShm(wcl::ShmReader* shm) { shm_ = shm; }

private:
    HRESULT DeliverSample(IUnknown* token);
    HRESULT AcquireSample(IMFSample** sample, IMFMediaBuffer** buffer);
    void PrepareAllocator();
    void FillPlaceholder(std::vector<uint8_t>& buffer) const;

    VirtualCameraSource* source_ = nullptr;
    ComPtr<IMFStreamDescriptor> descriptor_;
    ComPtr<IMFMediaEventQueue> events_;
    ComPtr<wcl::LoggingAttributes> stream_attributes_;
    ComPtr<IMFMediaType> media_type_;
    UINT32 width_ = 1920;
    UINT32 height_ = 1080;
    UINT32 frame_bytes_ = 1920 * 1080 * 3 / 2;
    LONGLONG frame_duration_ = 333333;
    ComPtr<IMFVideoSampleAllocator> allocator_;
    wcl::ShmReader* shm_ = nullptr;
    std::mutex mutex_;
    MF_STREAM_STATE state_ = MF_STREAM_STATE_STOPPED;
    UINT32 delivered_ = 0;
    std::vector<uint8_t> frame_;
    bool request_logged_ = false;
    bool size_mismatch_logged_ = false;
};

/// The virtual camera media source.
class VirtualCameraSource
    : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IMFMediaSourceEx,
                          IMFSampleAllocatorControl, IMFGetService, wcl::IKsControlLocal> {
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
        } else if (IsEqualIID(riid, __uuidof(wcl::IKsControlLocal))) {
            found = static_cast<wcl::IKsControlLocal*>(this);
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

    HRESULT Initialize(IMFAttributes* activate_attributes) {
        config_ = wcl::LoadVideoConfig();

        // Source attributes: the Frame Server reads these (MF_E_ATTRIBUTENOTFOUND
        // otherwise) to recognise a video capture device source. The store is
        // wrapped so every lookup it makes is visible in the log.
        source_attributes_ = Microsoft::WRL::Make<wcl::LoggingAttributes>(L"source-store");
        if (!source_attributes_ || !source_attributes_->Store()) {
            return E_OUTOFMEMORY;
        }
        IMFAttributes* store = source_attributes_->Store();

        // Whatever the Frame Server wrote into the activate's store is part of
        // the source's identity too, so it is carried over as is.
        if (activate_attributes) {
            activate_attributes->CopyAllItems(store);
        }
        store->SetGUID(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_CATEGORY, kVideoCameraCategory);
        store->SetString(MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, L"WebcamLink");
        store->SetUINT32(MF_SA_D3D_AWARE, 0);
        store->SetString(wcl::kConfigurationAppPackageFamilyName, wcl::kConfigurationAppName);

        // No associated-camera list: MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES
        // would ask the pipeline for the physical cameras this one wraps, and
        // this camera wraps none.

        // Sensor profile collection: the Camera app and other profile-aware
        // clients query it, and the Legacy profile is mandatory.
        ComPtr<IMFSensorProfileCollection> profiles;
        ComPtr<IMFSensorProfile> legacy;
        if (SUCCEEDED(MFCreateSensorProfileCollection(&profiles)) && profiles) {
            if (SUCCEEDED(MFCreateSensorProfile(wcl::kCameraProfileLegacy, 0, nullptr, &legacy)) &&
                legacy) {
                legacy->AddProfileFilter(0, L"((RES==;FRT<=30,1;SUT==))");
                profiles->AddProfile(legacy.Get());
            }
            store->SetUnknown(MF_DEVICEMFT_SENSORPROFILE_COLLECTION, profiles.Get());
        }

        // One NV12 video stream, sized by %ProgramData%\WebcamLink\config.ini.
        ComPtr<IMFMediaType> type;
        HRESULT hr = MFCreateMediaType(&type);
        if (FAILED(hr)) {
            return hr;
        }
        type->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video);
        type->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_NV12);
        MFSetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, config_.width, config_.height);
        MFSetAttributeRatio(type.Get(), MF_MT_FRAME_RATE, config_.fps, 1);
        MFSetAttributeRatio(type.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1);
        type->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive);
        // The Frame Server reads these; a missing one surfaces as
        // MF_E_ATTRIBUTENOTFOUND while it sets up the stream.
        type->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE);
        type->SetUINT32(MF_MT_FIXED_SIZE_SAMPLES, TRUE);
        type->SetUINT32(MF_MT_COMPRESSED, FALSE);
        type->SetUINT32(MF_MT_SAMPLE_SIZE, config_.width * config_.height * 3 / 2);
        type->SetUINT32(MF_MT_DEFAULT_STRIDE, config_.width);
        type->SetUINT32(MF_MT_AVG_BITRATE, config_.bitrate_kbps * 1000);

        IMFMediaType* types[] = {type.Get()};
        hr = MFCreateStreamDescriptor(0, 1, types, &stream_descriptor_);
        if (FAILED(hr)) {
            return hr;
        }
        // The descriptor needs a current media type: clients and the pipeline's
        // sample allocator read it.
        ComPtr<IMFMediaTypeHandler> handler;
        hr = stream_descriptor_->GetMediaTypeHandler(&handler);
        if (SUCCEEDED(hr) && handler) {
            handler->SetCurrentMediaType(type.Get());
        }

        camera_stream_ = Microsoft::WRL::Make<CameraStream>(this, stream_descriptor_.Get(),
                                                            type.Get(), config_);
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
        wcl::LogLine(L"source::GetCharacteristics -> IS_LIVE|CAN_PAUSE");
        return S_OK;
    }
    STDMETHODIMP CreatePresentationDescriptor(IMFPresentationDescriptor** descriptor) override {
        if (!descriptor) {
            return E_POINTER;
        }
        const HRESULT hr = presentation_descriptor_.CopyTo(descriptor);
        wchar_t line[160];
        swprintf_s(line, L"source::CreatePresentationDescriptor -> 0x%08lX",
                   static_cast<unsigned long>(hr));
        wcl::LogLine(line);
        return hr;
    }
    STDMETHODIMP Start(IMFPresentationDescriptor* descriptor, const GUID* format,
                       const PROPVARIANT* position) override {
        wchar_t line[192];
        swprintf_s(line, L"source::Start(state=%s, descriptor=%s)", StateName(state_),
                   descriptor ? L"set" : L"nul");
        wcl::LogLine(line);
        const HRESULT hr = StartSource(descriptor);
        swprintf_s(line, L"source::Start -> 0x%08lX", static_cast<unsigned long>(hr));
        wcl::LogLine(line);
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
        const HRESULT hr = source_attributes_.CopyTo(attributes);
        UINT32 count = 0;
        if (SUCCEEDED(hr) && *attributes) {
            (*attributes)->GetCount(&count);
        }
        wchar_t line[192];
        swprintf_s(line, L"source::GetSourceAttributes -> 0x%08lX (%u items)",
                   static_cast<unsigned long>(hr), count);
        wcl::LogLine(line);
        return hr;
    }
    STDMETHODIMP GetStreamAttributes(DWORD stream_id, IMFAttributes** attributes) override {
        if (!attributes) {
            return E_POINTER;
        }
        HRESULT hr = MF_E_INVALIDSTREAMNUMBER;
        UINT32 count = 0;
        if (stream_id == 0) {
            hr = camera_stream_->StreamAttributes()->QueryInterface(IID_PPV_ARGS(attributes));
            if (SUCCEEDED(hr) && *attributes) {
                (*attributes)->GetCount(&count);
            }
        }
        wchar_t line[192];
        swprintf_s(line, L"source::GetStreamAttributes(stream=%lu) -> 0x%08lX (%u items)",
                   static_cast<unsigned long>(stream_id), static_cast<unsigned long>(hr), count);
        wcl::LogLine(line);
        return hr;
    }
    STDMETHODIMP SetD3DManager(IUnknown* manager) override { return S_OK; }

    // IMFSampleAllocatorControl — the Frame Server hands over the allocator it
    // shares between clients and expects those buffers back; our own buffers
    // stay as a fallback for clients that never set one.
    STDMETHODIMP SetDefaultAllocator(DWORD stream_id, IUnknown* allocator) override {
        HRESULT hr = E_POINTER;
        ComPtr<IMFVideoSampleAllocator> provided;
        if (stream_id != 0) {
            hr = MF_E_INVALIDSTREAMNUMBER;
        } else if (allocator) {
            hr = allocator->QueryInterface(IID_PPV_ARGS(&provided));
        }
        if (SUCCEEDED(hr)) {
            camera_stream_->SetSampleAllocator(provided.Get());
        }
        wchar_t line[160];
        swprintf_s(line, L"source::SetDefaultAllocator(stream=%lu) -> 0x%08lX",
                   static_cast<unsigned long>(stream_id), static_cast<unsigned long>(hr));
        wcl::LogLine(line);
        return hr;
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
            *usage = MFSampleAllocatorUsage_UsesProvidedAllocator;
        }
        wcl::LogLine(L"source::GetAllocatorUsage -> UsesProvidedAllocator");
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

    // IKsControl — the Frame Server treats a video capture source as a KS filter:
    // the interface has to be there, and KS operations answer the way a driver
    // without the property does.
    STDMETHODIMP KsProperty(void*, ULONG, void*, ULONG, ULONG*) override {
        return KsNotSupported(L"KsProperty");
    }
    STDMETHODIMP KsMethod(void*, ULONG, void*, ULONG, ULONG*) override {
        return KsNotSupported(L"KsMethod");
    }
    STDMETHODIMP KsEvent(void*, ULONG, void*, ULONG, ULONG*) override {
        return KsNotSupported(L"KsEvent");
    }

    wcl::ShmReader* Shm() { return &shm_; }
    MF_STREAM_STATE State() { return state_; }

private:
    /// Start() body, split out so the wrapper logs every exit path. Starting an
    /// already running source is allowed: the pipeline restarts streams between
    /// clients, and refusing that transition fails the client's first read.
    HRESULT StartSource(IMFPresentationDescriptor* descriptor) {
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
        if (!shm_.Open(config_.width, config_.height, config_.fps)) {
            return MF_E_SHUTDOWN;
        }
        state_ = MF_STREAM_STATE_RUNNING;
        camera_stream_->SetStreamStateStateOnly(MF_STREAM_STATE_RUNNING);

        // Hand the stream over: MENewStream carries the stream object itself and
        // is the only way a client ever obtains our IMFMediaStream.
        hr = events_->QueueEventParamUnk(MENewStream, GUID_NULL, S_OK, camera_stream_.Get());
        if (SUCCEEDED(hr)) {
            // Live source: the start time is "now", the pipeline reads it from
            // the MESourceStarted event.
            PROPVARIANT start_time{};
            start_time.vt = VT_I8;
            start_time.hVal.QuadPart = MFGetSystemTime();
            hr = events_->QueueEventParamVar(MESourceStarted, GUID_NULL, S_OK, &start_time);
        }
        if (SUCCEEDED(hr)) {
            hr = camera_stream_->QueueEvent(MEStreamStarted, GUID_NULL, S_OK, nullptr);
        }
        return hr;
    }

    ComPtr<IMFStreamDescriptor> stream_descriptor_;
    ComPtr<IMFPresentationDescriptor> presentation_descriptor_;
    ComPtr<IMFMediaEventQueue> events_;
    ComPtr<wcl::LoggingAttributes> source_attributes_;
    ComPtr<CameraStream> camera_stream_;
    wcl::VideoConfig config_;
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
    HRESULT hr = MF_E_MEDIA_SOURCE_NOT_STARTED;
    if (state_ == MF_STREAM_STATE_RUNNING) {
        hr = DeliverSample(token);
    }
    // Logged once: the Frame Server pumps samples at frame rate.
    if (!request_logged_) {
        request_logged_ = true;
        wchar_t line[160];
        swprintf_s(line, L"stream::RequestSample(%s) -> 0x%08lX", StateName(state_),
                   static_cast<unsigned long>(hr));
        wcl::LogLine(line);
    }
    return hr;
}

/// Initialises the pipeline's allocator for the current media type, if one was
/// handed over. A refusal is not fatal: our own buffers take over.
void CameraStream::PrepareAllocator() {
    if (!allocator_ || !media_type_) {
        return;
    }
    const HRESULT hr = allocator_->InitializeSampleAllocator(kAllocatedSamples, media_type_.Get());
    if (FAILED(hr)) {
        wchar_t line[160];
        swprintf_s(line, L"stream::InitializeSampleAllocator -> 0x%08lX (own buffers)",
                   static_cast<unsigned long>(hr));
        wcl::LogLine(line);
        allocator_.Reset();
        return;
    }
    wcl::LogLine(L"stream::InitializeSampleAllocator -> OK");
}

/// A sample to fill: from the pipeline's allocator when it works, from our own
/// memory buffer otherwise.
HRESULT CameraStream::AcquireSample(IMFSample** sample, IMFMediaBuffer** buffer) {
    if (allocator_) {
        ComPtr<IMFSample> provided_sample;
        HRESULT hr = allocator_->AllocateSample(&provided_sample);
        if (SUCCEEDED(hr) && provided_sample) {
            ComPtr<IMFMediaBuffer> provided_buffer;
            hr = provided_sample->GetBufferByIndex(0, &provided_buffer);
            if (SUCCEEDED(hr) && provided_buffer) {
                *sample = provided_sample.Detach();
                *buffer = provided_buffer.Detach();
                return S_OK;
            }
        }
        wchar_t line[160];
        swprintf_s(line, L"stream::AllocateSample -> 0x%08lX (own buffer)",
                   static_cast<unsigned long>(hr));
        wcl::LogLine(line);
    }

    ComPtr<IMFMediaBuffer> own_buffer;
    HRESULT hr = MFCreateMemoryBuffer(frame_bytes_, &own_buffer);
    if (FAILED(hr)) {
        return hr;
    }
    ComPtr<IMFSample> own_sample;
    hr = MFCreateSample(&own_sample);
    if (SUCCEEDED(hr)) {
        hr = own_sample->AddBuffer(own_buffer.Get());
    }
    if (FAILED(hr)) {
        return hr;
    }
    *sample = own_sample.Detach();
    *buffer = own_buffer.Detach();
    return S_OK;
}

HRESULT CameraStream::DeliverSample(IUnknown* token) {
    if (source_->State() != MF_STREAM_STATE_RUNNING) {
        return MF_E_MEDIA_SOURCE_NOT_STARTED;
    }

    // Live source: never wait for the producer, the pipeline paces us. When
    // nothing new is ready the previous frame is repeated (a placeholder on the
    // first request), which keeps the stream at its own frame rate.
    std::vector<uint8_t> incoming;
    UINT32 width = 0;
    UINT32 height = 0;
    const bool has_frame = shm_ && shm_->ReadFrame(incoming, width, height, 0);
    const bool size_matches =
        width == width_ && height == height_ && incoming.size() == static_cast<size_t>(frame_bytes_);
    if (has_frame && !size_matches && !size_mismatch_logged_) {
        size_mismatch_logged_ = true;
        wchar_t line[192];
        swprintf_s(line, L"stream: producteur %ux%u ≠ format annoncé %ux%u (image de remplacement)",
                   width, height, width_, height_);
        wcl::LogLine(line);
    }
    if (has_frame && size_matches) {
        frame_.swap(incoming);
    }
    if (frame_.size() != static_cast<size_t>(frame_bytes_)) {
        FillPlaceholder(frame_);
    }

    ComPtr<IMFSample> sample;
    ComPtr<IMFMediaBuffer> buffer;
    HRESULT hr = AcquireSample(&sample, &buffer);
    if (FAILED(hr)) {
        return hr;
    }

    // Copy the frame, honouring the buffer's own stride when it is 2D.
    ComPtr<IMF2DBuffer2> buffer2d;
    if (SUCCEEDED(buffer.As(&buffer2d))) {
        BYTE* row = nullptr;
        LONG pitch = 0;
        BYTE* start = nullptr;
        DWORD length = 0;
        hr = buffer2d->Lock2DSize(MF2DBuffer_LockFlags_Write, &row, &pitch, &start, &length);
        if (FAILED(hr)) {
            return hr;
        }
        for (UINT32 y = 0; y < height_; ++y) {
            memcpy(row + static_cast<ptrdiff_t>(y) * pitch, frame_.data() + y * width_, width_);
        }
        for (UINT32 y = 0; y < height_ / 2; ++y) {
            memcpy(row + static_cast<ptrdiff_t>(height_ + y) * pitch,
                   frame_.data() + width_ * height_ + y * width_, width_);
        }
        buffer2d->Unlock2D();
    } else {
        BYTE* data = nullptr;
        hr = buffer->Lock(&data, nullptr, nullptr);
        if (FAILED(hr)) {
            return hr;
        }
        memcpy(data, frame_.data(), frame_bytes_);
        buffer->Unlock();
    }
    buffer->SetCurrentLength(frame_bytes_);

    // Live source: samples carry the same clock as MESourceStarted, otherwise
    // the pipeline sees them as stale and drops them (the client only gets
    // stream ticks).
    hr = sample->SetSampleTime(MFGetSystemTime());
    if (SUCCEEDED(hr)) {
        hr = sample->SetSampleDuration(frame_duration_);
    }
    if (FAILED(hr)) {
        return hr;
    }
    if (token) {
        sample->SetUnknown(MFSampleExtension_Token, token);
    }
    delivered_++;

    return events_->QueueEventParamUnk(MEMediaSample, GUID_NULL, S_OK, sample.Get());
}

void CameraStream::FillPlaceholder(std::vector<uint8_t>& buffer) const {
    buffer.resize(frame_bytes_);
    // Neutral dark gradient: clearly an idle camera, not a video image.
    uint8_t* y = buffer.data();
    for (UINT32 row = 0; row < height_; ++row) {
        const uint8_t value = static_cast<uint8_t>(24 + (row * 40) / height_);
        memset(y + row * width_, value, width_);
    }
    memset(y + width_ * height_, 128, frame_bytes_ - width_ * height_);
}

}  // namespace

namespace wcl {

HRESULT CreateVirtualCamera(IMFAttributes* activate_attributes, IUnknown** source) {
    if (!source) {
        return E_POINTER;
    }
    auto camera = Microsoft::WRL::Make<VirtualCameraSource>();
    if (!camera) {
        return E_OUTOFMEMORY;
    }
    HRESULT hr = camera->Initialize(activate_attributes);
    if (FAILED(hr)) {
        return hr;
    }
    return camera.CopyTo(source);
}

}  // namespace wcl
