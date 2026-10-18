#pragma once

#include <windows.h>
#include <unknwn.h>

struct IMFAttributes;

namespace wcl {

// Virtual camera attribute (mfvirtualcamera.h hides these behind
// WINAPI_PARTITION_APP, so the values are repeated here).
// {28A5531A-57DD-4FD5-AAA7-385ABF57D785}
static const GUID kEnableMsCameraEffects = {
    0x28a5531a, 0x57dd, 0x4fd5, {0xaa, 0xa7, 0x38, 0x5a, 0xbf, 0x57, 0xd7, 0x85}};
// KSCAMERAPROFILE_Legacy (ksmedia.h): the profile every video capture source
// publishes. Without it, profile-aware Frame Server components fail the source
// setup with MF_E_ATTRIBUTENOTFOUND.
// {B4894D81-62B7-4EEC-8740-80658C4A9D3E}
static const GUID kCameraProfileLegacy = {
    0xb4894d81, 0x62b7, 0x4eec, {0x87, 0x40, 0x80, 0x65, 0x8c, 0x4a, 0x9d, 0x3e}};
// PINNAME_VIDEO_CAPTURE (ksmedia.h): stream category of a video capture pin,
// expected on the stream attributes and on the stream descriptor.
// {FB6C4281-0353-11D1-905F-0000C0CC16BA}
static const GUID kPinNameVideoCapture = {
    0xfb6c4281, 0x0353, 0x11d1, {0x90, 0x5f, 0x00, 0x00, 0xc0, 0xcc, 0x16, 0xba}};
// MF_VIRTUALCAMERA_CONFIGURATION_APP_PACKAGE_FAMILY_NAME (mfvirtualcamera.h):
// the pipeline reads it from the camera, the activate and the source attributes
// and hands a failed lookup back as IMFVirtualCamera::Start's result.
// {658ABE51-8044-462E-97EA-E676FD72055F}
static const GUID kConfigurationAppPackageFamilyName = {
    0x658abe51, 0x8044, 0x462e, {0x97, 0xea, 0xe6, 0x76, 0xfd, 0x72, 0x05, 0x5f}};
/// IKsControl, declared here instead of including ks.h (that header redefines
/// identifiers the Media Foundation headers need). The Frame Server treats a
/// video capture source as a KS filter and asks for this interface while it
/// sets the camera up; KS parameters only have to be pointer sized for the
/// vtable to line up.
// {28F54685-06FD-11D2-B27A-00A0C9223196}
MIDL_INTERFACE("28F54685-06FD-11D2-B27A-00A0C9223196")
IKsControlLocal : public IUnknown {
public:
    virtual HRESULT STDMETHODCALLTYPE KsProperty(void* property, ULONG property_length, void* data,
                                                ULONG data_length, ULONG* bytes_returned) = 0;
    virtual HRESULT STDMETHODCALLTYPE KsMethod(void* method, ULONG method_length, void* data,
                                              ULONG data_length, ULONG* bytes_returned) = 0;
    virtual HRESULT STDMETHODCALLTYPE KsEvent(void* event, ULONG event_length, void* data,
                                             ULONG data_length, ULONG* bytes_returned) = 0;
};

/// Camera Settings launch point. No packaged configuration app exists yet, so
/// a placeholder stands in for the package family name.
static const wchar_t* kConfigurationAppName = L"WebcamLink_9p1cm7s6z8q4k";

/// Creates the WebcamLink virtual camera media source (IMFMediaSourceEx).
/// Everything the Frame Server wrote into the activate's store is carried over
/// to the source attributes, the way the reference implementation does it.
HRESULT CreateVirtualCamera(IMFAttributes* activate_attributes, IUnknown** source);

/// Creates the IMFActivate the Frame Server instantiates from our class
/// factory; it builds the media source on demand.
HRESULT CreateCameraActivate(IUnknown** activate);

/// Appends a line to %TEMP%\webcamlink-vcam.log (the DLL runs inside the
/// Frame Server, a file is the only observable output).
void LogLine(const wchar_t* message);

}  // namespace wcl
